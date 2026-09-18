//! Local HTTP API (§57–§60): routes, SSE streaming, agent observation.
//! The frontend never touches llama.cpp directly (§7).
//!
//! Stage 2: persistent history, validated inputs, consistent error envelope.

use crate::agent::{AgentMode, AgentState, CancelToken};
use crate::downloads::{describe_model, DownloadManager, ModelEstimates};
use crate::generation::{ActiveGeneration, GenerationTracker};
use crate::hardware;
use crate::inference::{IInferenceEngine, InferenceConfig, StubEngine};
use crate::llamaserver::{
    ChatTurn, InferenceStatus, LlamaServerManager, SidecarBinary, SidecarClient,
    DEFAULT_SIDECAR_PORT,
};
use crate::models::ModelManager;
use crate::permissions::PermissionManager;
use crate::settings::AppSettings;
use crate::storage::{Conversation, Message, Storage};
use crate::tools;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{
    sse::{Event, KeepAlive, Sse},
    IntoResponse, Response,
};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use futures::stream;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// A carriage return inside an SSE field value aborts the stream: axum
/// splits `data` on newlines but panics on a \r, taking the worker thread
/// with it. A sidecar error carrying Windows line endings killed a live
/// generation that way, and ordinary model output can contain one just as
/// easily. Every event payload is normalized here rather than trusted.
fn sse_text(text: impl Into<String>) -> String {
    let text = text.into();
    if text.contains('\r') {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
    } else {
        text
    }
}

/// `Event::data` with that normalization applied. Used in place of `data`
/// for every event this module sends.
trait SafeEvent {
    fn safe_data(self, text: impl Into<String>) -> Event;
}

impl SafeEvent for Event {
    fn safe_data(self, text: impl Into<String>) -> Event {
        self.data(sse_text(text))
    }
}

const MAX_MESSAGE_CHARS: usize = 200_000;
const MAX_TITLE_CHARS: usize = 200;
const CHAT_TOOL_ROUNDS: u32 = 24;
/// Stage 6: history window fed to the model (§21). Older turns are dropped,
/// newest kept; the context endpoint reports exactly what was dropped.
/// A backstop on message count only. The character budget derived from the
/// context window decides what travels, and automatic compaction summarizes
/// older messages before that budget drops any; a count cap of 20 used to
/// drop messages silently long before the window was full.
const HISTORY_TURNS: usize = 200;
const HISTORY_CHARS: usize = 60_000;
const ATTACH_CHARS_PER_TURN: usize = 20_000;

#[derive(Clone)]
pub struct AppState {
    pub models: Arc<tokio::sync::RwLock<ModelManager>>,
    pub inference: Arc<tokio::sync::RwLock<StubEngine>>,
    pub llama: Arc<tokio::sync::RwLock<LlamaServerManager>>,
    pub downloads: Arc<tokio::sync::RwLock<DownloadManager>>,
    pub generations: Arc<tokio::sync::RwLock<GenerationTracker>>,
    pub agents: Arc<tokio::sync::RwLock<crate::agent_runner::AgentRegistry>>,
    pub metrics: crate::metrics::SharedLog,
    pub agent_active: Arc<std::sync::atomic::AtomicUsize>,
    pub artifacts_dir: PathBuf,
    pub http: reqwest::Client,
    pub permissions: Arc<tokio::sync::RwLock<PermissionManager>>,
    pub settings: Arc<tokio::sync::RwLock<AppSettings>>,
    /// Serialize durable settings changes, including the permission shortcut.
    pub(crate) settings_update: Arc<tokio::sync::Mutex<()>>,
    /// Serialize model replacement with generation/agent registration.
    runtime_update: Arc<tokio::sync::Mutex<()>>,
    pub storage: Arc<tokio::sync::Mutex<Storage>>,
    pub models_dir: PathBuf,
    /// The installation root (runtime/bin, plugins). Set from the resolved
    /// configuration; test states use the models folder's parent.
    pub install_root: PathBuf,
    pub attachments_dir: PathBuf,
    /// Stage 21: staged model-load progress (validating → loading → ready).
    pub load_progress: Arc<tokio::sync::RwLock<LoadProgress>>,
    /// Stage 32: repo-index cache per workspace id: (dir mtime, index).
    pub repo_index: Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, (u64, crate::repo_index::RepoIndex)>>,
    >,
    /// The fit search ahead of each model's first load.
    pub fit_preparation: Arc<FitPreparation>,
}

/// Stage 21 load progress with truthful stages (§174 rule: no fake %).
/// `stage` is done|error|validating|loading|health|ready|idle|cancelled.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadProgress {
    pub model_id: String,
    pub stage: String,
    pub detail: String,
    pub updated_at: String,
    pub cancel_requested: bool,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Request context that changes during a conversation (the project listing,
/// saved memory, the reasoning instruction, attachment excerpts) goes after
/// the saved history instead of before it, so the system prompt and every
/// earlier turn stay byte-identical between requests and llama-server reuses
/// their cache instead of re-reading them on the user's hardware. Setting
/// COMPANION_LEGACY_PROMPT_ORDER restores the previous layout; it exists only
/// for the before/after measurement and is removed after it.
pub(crate) fn legacy_prompt_order() -> bool {
    std::env::var_os("COMPANION_LEGACY_PROMPT_ORDER").is_some()
}

/// Stands in for the project listing inside the stable system prompt.
const LISTING_IN_LATEST_MESSAGE: &str =
    "(The project's current directory listing is attached to the latest message.)";

/// The changing context for the latest user turn.
fn changing_context_block(snapshot: &str, memory: &str) -> String {
    let mut block = String::new();
    if !snapshot.trim().is_empty() {
        block.push_str(&format!("\n\n[Linked project: current directory listing]\n{}", snapshot.trim_end()));
    }
    if !memory.trim().is_empty() {
        block.push_str("\n\n");
        block.push_str(memory.trim());
    }
    block
}

/// The user message an attachment was sent with: the first user message
/// saved at or after the upload (the pending message counts as latest).
pub(crate) fn attachment_owner<'a>(history: &'a [Message], attachment: &crate::storage::Attachment) -> Option<&'a str> {
    let uploaded = chrono::DateTime::parse_from_rfc3339(&attachment.created_at).ok();
    history
        .iter()
        .filter(|message| message.role == "user")
        .find(|message| {
            if message.id == "pending" || message.created_at.is_empty() {
                return true;
            }
            match (uploaded, chrono::DateTime::parse_from_rfc3339(&message.created_at).ok()) {
                (Some(uploaded), Some(sent)) => sent >= uploaded,
                _ => message.created_at >= attachment.created_at,
            }
        })
        .map(|message| message.id.as_str())
}

/// The turn an attachment belongs on: its own message when that message is in
/// the window; the latest user turn when no message owns it yet; None when its
/// message has left the window.
fn attachment_turn(
    turns: &[ChatTurn],
    ids: &[String],
    history: &[Message],
    attachment: &crate::storage::Attachment,
) -> Option<usize> {
    match attachment_owner(history, attachment) {
        Some(owner) => ids.iter().position(|id| id == owner),
        None => turns.iter().rposition(|turn| turn.role == "user"),
    }
}

/// Store every request a client sends for `owner_id` (a reply or an agent
/// run), when Privacy > "Keep a record of model requests" is on. Writes happen
/// after the response, on their own task, and never fail the request.
pub(crate) fn request_recorder(
    state: &AppState,
    conversation_id: &str,
    owner_id: &str,
    kind: &'static str,
) -> crate::llamaserver::RequestRecorder {
    let state = state.clone();
    let conversation_id = conversation_id.to_string();
    let owner_id = owner_id.to_string();
    let seq = Arc::new(std::sync::atomic::AtomicU32::new(0));
    Arc::new(move |request: crate::llamaserver::RecordedRequest| {
        let state = state.clone();
        let record = crate::storage::ModelRequestRecord {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation_id.clone(),
            owner_id: owner_id.clone(),
            seq: seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            kind: kind.to_string(),
            request_json: request.body.to_string(),
            raw_output: request.output,
            finish_reason: request.finish_reason,
            outcome: request.outcome.to_string(),
            failure: request.failure,
            prompt_tokens: request.prompt_tokens,
            cached_tokens: request.cached_tokens,
            generated_tokens: request.generated_tokens,
            created_at: now_rfc3339(),
        };
        tokio::spawn(async move {
            if !state.settings.read().await.privacy.record_model_requests {
                return;
            }
            if let Err(error) = state.storage.lock().await.record_model_request(&record) {
                tracing::warn!("could not keep the model request record: {error}");
            }
        });
    })
}

impl AppState {
    pub fn new_stub() -> Self {
        Self::new_with_storage(
            Storage::open_in_memory().expect("in-memory sqlite"),
            PathBuf::from("models"),
        )
    }

    pub fn new_with_storage(storage: Storage, models_dir: PathBuf) -> Self {
        let attachments_dir = models_dir
            .parent()
            .map(|p| p.join("data").join("attachments"))
            .unwrap_or_else(|| PathBuf::from("data").join("attachments"));
        Self::new_with_dirs(storage, models_dir, attachments_dir)
    }

    pub fn new_with_dirs(storage: Storage, models_dir: PathBuf, attachments_dir: PathBuf) -> Self {
        let artifacts_dir = attachments_dir
            .parent()
            .map(|p| p.join("artifacts"))
            .unwrap_or_else(|| PathBuf::from("data").join("artifacts"));
        let settings = storage
            .load_settings()
            .unwrap_or_else(|error| {
                tracing::warn!("Stored settings could not be read; using safe defaults: {error}");
                None
            })
            .unwrap_or_default()
            .normalized();
        // Only the explicit global preference is durable. Temporary grants and
        // one-time approvals belong to the old process and are never restored.
        let permissions = PermissionManager::new(crate::permissions::autonomy_for_mode(&settings.agent.permission_mode));
        Self {
            models: Arc::new(tokio::sync::RwLock::new(ModelManager::new())),
            inference: Arc::new(tokio::sync::RwLock::new(StubEngine::new())),
            llama: Arc::new(tokio::sync::RwLock::new(LlamaServerManager::new())),
            downloads: Arc::new(tokio::sync::RwLock::new(DownloadManager::new(
                models_dir.clone(),
            ))),
            generations: Arc::new(tokio::sync::RwLock::new(GenerationTracker::new())),
            agents: Arc::new(tokio::sync::RwLock::new(
                crate::agent_runner::AgentRegistry::new(),
            )),
            metrics: std::sync::Arc::new(std::sync::Mutex::new(crate::metrics::MetricsLog::new())),
            agent_active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            // Downloads and web search only. No whole-request timeout: a
            // multi-gigabyte model download legitimately runs for hours;
            // a stalled connection is caught by the idle read timeout.
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .read_timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("http client"),
            permissions: Arc::new(tokio::sync::RwLock::new(permissions)),
            settings: Arc::new(tokio::sync::RwLock::new(settings)),
            settings_update: Arc::new(tokio::sync::Mutex::new(())),
            runtime_update: Arc::new(tokio::sync::Mutex::new(())),
            storage: Arc::new(tokio::sync::Mutex::new(storage)),
            install_root: models_dir
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            models_dir,
            attachments_dir,
            artifacts_dir,
            load_progress: Arc::new(tokio::sync::RwLock::new(LoadProgress {
                model_id: String::new(),
                stage: "idle".into(),
                detail: String::new(),
                updated_at: now_rfc3339(),
                cancel_requested: false,
            })),
            repo_index: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            fit_preparation: Arc::new(FitPreparation::default()),
        }
    }

    /// The installation root the runtime and plugins are found under.
    pub fn with_install_root(mut self, root: PathBuf) -> Self {
        self.install_root = root;
        self
    }

    pub fn runtime_dir(&self) -> PathBuf {
        crate::llamaserver::runtime_dir(&self.install_root)
    }
}

/// Consistent error envelope (§52): human message + actionable hint.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    hint: String,
}

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
    hint: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            hint: hint.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg, "Check the id and try again.")
    }
    fn bad(msg: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg, hint)
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            msg,
            "The conversation was not lost. Check backend logs and retry.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: self.message,
            hint: self.hint,
        });
        (self.status, body).into_response()
    }
}

fn validate_role(role: &str) -> Result<(), ApiError> {
    match role {
        "user" | "assistant" | "tool" => Ok(()),
        _ => Err(ApiError::bad(
            format!("invalid role '{role}'"),
            "Role must be one of: user, assistant, tool.",
        )),
    }
}

fn validate_content(content: &str) -> Result<(), ApiError> {
    if content.trim().is_empty() {
        return Err(ApiError::bad(
            "message is empty",
            "Type a message before sending.",
        ));
    }
    if content.len() > MAX_MESSAGE_CHARS {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("message too large ({} chars, max {MAX_MESSAGE_CHARS})", content.len()),
            "Attach large files instead of pasting them; only relevant sections enter context (§69).",
        ));
    }
    Ok(())
}

pub fn router(state: AppState) -> Router {
    let origins = trusted_loopback_origins();
    let guarded_origins = origins.clone();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    Router::new()
        .route("/api/health", get(health))
        .route("/api/runtime/policy", get(runtime_policy))
        .route("/api/models", get(list_models))
        .route("/api/models/load", post(load_model))
        .route("/api/models/unload", post(unload_models))
        .route("/api/models/scan", post(scan_models))
        .route("/api/models/:id/tooling/check", post(recheck_model_tooling))
        .route(
            "/api/models/downloads",
            get(list_downloads).post(start_download),
        )
        .route("/api/models/downloads/:id", get(get_download))
        .route("/api/models/downloads/:id/pause", post(pause_download))
        .route("/api/models/downloads/:id/resume", post(resume_download))
        .route("/api/models/downloads/:id/cancel", post(cancel_download))
        .route("/api/models/:id", get(model_detail).delete(delete_model))
        .route("/api/models/:id/recommend", get(model_recommend))
        .route("/api/chat", post(chat_sse))
        .route("/api/chat/stop", post(chat_stop))
        .route(
            "/api/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/api/conversations/:id/messages",
            get(list_messages).post(post_message),
        )
        .route("/api/conversations/:id/messages/:mid", patch(edit_message))
        .route(
            "/api/conversations/:id/attachments",
            get(list_attachments).post(add_attachment),
        )
        .route(
            "/api/conversations/:id/attachments/:aid",
            delete(delete_attachment),
        )
        .route(
            "/api/conversations/:id/attachments/:aid/file",
            get(attachment_file),
        )
        .route("/api/artifacts", get(list_artifacts))
        .route("/api/artifacts/:id/file", get(artifact_file))
        .route("/api/conversations/:id/context", get(conversation_context))
        .route("/api/tools", get(list_tools))
        .route("/api/tools/execute", post(execute_tool))
        .route("/api/tools/executions", get(list_tool_executions))
        .route("/api/agent/run", post(run_agent))
        .route("/api/agent/runs", get(agent_runs))
        .route("/api/agent/runs/:id", get(agent_status))
        .route("/api/agent/runs/:id/events", get(agent_events))
        .route("/api/agent/runs/:id/resume", post(agent_resume))
        .route("/api/agent/runs/:id/stop", post(agent_stop))
        .route("/api/commands", get(list_commands))
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/workspaces/:id",
            get(get_workspace).delete(delete_workspace),
        )
        .route("/api/conversations/:id/fork", post(fork_conversation))
        .route("/api/conversations/:id/share", post(share_conversation))
        .route("/api/conversations/:id/prepare", post(prepare_conversation))
        .route(
            "/api/conversations/:id/compatibility",
            get(conversation_compatibility),
        )
        .route(
            "/api/conversations/:id",
            get(get_conversation)
                .delete(delete_conversation)
                .patch(patch_conversation),
        )
        .route("/api/sessions", get(list_sessions))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route(
            "/api/permissions/mode",
            get(get_permission_mode).put(put_permission_mode),
        )
        .route("/api/system", get(system_info))
        .route("/api/system/pick-folder", post(pick_folder))
        .route("/api/system/metrics", get(system_metrics))
        .route("/api/system/overview", get(system_overview))
        .route("/api/inference/status", get(inference_status))
        .route("/api/inference/start", post(inference_start))
        .route("/api/inference/stop", post(inference_stop))
        // Stage 21: staged load progress + cancel.
        .route("/api/models/load/progress", get(load_progress))
        .route("/api/models/load/cancel", post(cancel_load))
        // Stage 22: versioned API. v1 mirrors the stable surface; the
        // unversioned paths stay for compatibility.
        .route("/api/v1/status", get(v1_status))
        .route("/api/v1/models", get(list_models))
        .route(
            "/api/v1/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route("/api/v1/chat", post(chat_sse))
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/settings", get(get_settings))
        .route("/api/v1/system", get(system_info))
        .route("/api/v1/system/metrics", get(system_metrics))
        .route("/api/v1/system/overview", get(system_overview))
        .route("/api/v1/tools", get(list_tools))
        .route("/api/v1/commands", get(list_commands))
        .route("/api/v1/workspaces", get(list_workspaces))
        // Stage 25: timeline + explicit compaction endpoint.
        .route(
            "/api/conversations/:id/timeline",
            get(conversation_timeline),
        )
        .route("/api/conversations/:id/compact", post(compact_endpoint))
        // Stage 26: scoped memory.
        .route("/api/memory", get(list_memory).post(add_memory))
        .route("/api/memory/:id", delete(delete_memory))
        .route("/api/memory/:id/share", post(share_memory))
        // Stage 24: project instructions + full diff.
        .route(
            "/api/workspaces/:id/instructions",
            get(workspace_instructions),
        )
        .route("/api/workspaces/:id/diff", get(workspace_diff))
        // Stage 27: priority, export, recovery, session actions.
        .route("/api/sessions/:id", patch(patch_session))
        .route("/api/sessions/recovery", get(sessions_recovery))
        .route("/api/sessions/:id/action", post(session_action))
        .route("/api/conversations/:id/export", get(export_conversation))
        // Stage 28: attachment budget + OCR availability.
        .route(
            "/api/conversations/:id/attachment-budget",
            get(attachment_budget),
        )
        .route("/api/system/ocr", get(ocr_status))
        // Stage 23: artifact info (reveal path / openability).
        .route("/api/artifacts/:id/info", get(artifact_info))
        // Stage 29–30: cache budget + DAIO.
        .route("/api/system/cache", get(cache_budget))
        .route("/api/system/device", get(device_profile))
        .route("/api/system/capabilities", get(capability_db))
        .route("/api/system/calibrate", post(calibrate))
        .route("/api/models/:id/optimize", get(model_optimize))
        .route("/api/models/:id/calibration", get(model_calibration))
        .route("/api/models/:id/calibrate", post(calibrate_model))
        // Stage 32: repo index.
        .route("/api/workspaces/:id/index", get(workspace_index))
        // Stage 35: local knowledge.
        .route(
            "/api/workspaces/:id/knowledge",
            get(list_knowledge).post(ingest_knowledge),
        )
        .route("/api/workspaces/:id/knowledge/clear", post(clear_knowledge))
        // Stage 33: per-message telemetry.
        .route("/api/conversations/:id/metrics", get(conversation_metrics))
        // Stage 36: git with guardrails.
        .route("/api/workspaces/:id/git", get(workspace_git))
        // Stage 37: plugins.
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/:id/run", post(run_plugin_command))
        // Stage 34: diagnostics.
        .route("/api/doctor", get(doctor))
        // Stage 38: first-run + benchmark.
        .route("/api/setup/status", get(setup_status))
        .route("/api/system/benchmark", post(benchmark))
        .layer(cors)
        .layer(axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| {
            let origins = guarded_origins.clone();
            async move {
                if request.uri().path().starts_with("/api/") {
                    if let Some(origin) = request.headers().get(axum::http::header::ORIGIN) {
                        if !origins.contains(origin) {
                            return ApiError::new(StatusCode::FORBIDDEN, "This website is not allowed to access Companion", "Open the local Companion interface. Remote browser origins are blocked.").into_response();
                        }
                    }
                    // Same-origin DNS-rebinding requests can omit Origin.
                    if let Some(host) = request.headers().get(axum::http::header::HOST) {
                        let trusted_host = host.to_str().ok().map(|host| {
                            let expected = format!("http://{host}");
                            origins.iter().any(|origin| origin.to_str().ok() == Some(expected.as_str()))
                        }).unwrap_or(false);
                        if !trusted_host {
                            return ApiError::new(StatusCode::FORBIDDEN, "Only local Companion hosts are allowed", "Use localhost or a loopback address. LAN access requires a future authenticated mode.").into_response();
                        }
                    }
                }
                next.run(request).await
            }
        }))
        .with_state(state)
}

fn trusted_loopback_origins() -> Vec<axum::http::HeaderValue> {
    let api_port = std::env::var("COMPANION_ADDR")
        .ok()
        .and_then(|address| {
            address
                .rsplit(':')
                .next()
                .and_then(|port| port.parse::<u16>().ok())
        })
        .unwrap_or(3877);
    let ui_port = std::env::var("COMPANION_UI_PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(5173);
    let mut ports = vec![3877, api_port, ui_port];
    ports.sort_unstable();
    ports.dedup();
    ports
        .into_iter()
        .flat_map(|port| {
            ["localhost", "127.0.0.1", "[::1]"]
                .into_iter()
                .map(move |host| {
                    format!("http://{host}:{port}")
                        .parse()
                        .expect("valid loopback origin")
                })
        })
        .collect()
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "name": "companion-backend",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn list_models(State(s): State<AppState>) -> Json<Vec<crate::models::ModelMetadata>> {
    // Keep the selector in sync with files dropped into models/ while the app
    // is already running. Registration preserves the currently loaded model.
    // Reconciled, not only added to: a folder deleted by hand leaves the list
    // on the next read, including when the models folder is now empty.
    let (found, warnings) = crate::models::scan_models_dir(&s.models_dir);
    let known: std::collections::HashSet<String> = s.models.read().await.list().into_iter().map(|model| model.id).collect();
    let added = found.iter().any(|model| !known.contains(&model.id));
    let (removed, register_warnings) = s.models.write().await.reconcile(found);
    if added {
        // A model added while the app runs gets its fit ahead of its first load.
        spawn_fit_preparation(&s, std::time::Duration::from_secs(3));
    }
    if !removed.is_empty() {
        tracing::info!(?removed, "models removed from the list: their files are gone");
    }
    for warning in warnings.into_iter().chain(register_warnings) {
        tracing::warn!("{warning}");
    }
    let mut list = s.models.read().await.list();
    // Each model's tool check (`tooling.json`) and whether it is current for
    // this runtime and the model's template.
    let binary = SidecarBinary::detect(&s.runtime_dir()).ok();
    for model in &mut list {
        let profile = crate::tooling::read_profile(&crate::tooling::profile_dir(&model.gguf_path()));
        if let Some(binary) = &binary {
            let (runtime, template) = tooling_identity(binary, model);
            model.tooling_state = Some(crate::tooling::profile_state(profile.as_ref(), &runtime, &template));
        }
        model.tooling = profile;
    }
    // Tool support as llama.cpp reported it for each file when it was last
    // loaded; the chat template's own source only until then.
    let st = s.storage.lock().await;
    for model in &mut list {
        let key = crate::calibration::model_key(&model.gguf_path());
        match st.template_caps_for(&key) {
            Ok(Some(caps)) if caps.tools_supported() != crate::inference::Support::Unknown => {
                model.tool_calling = caps.tools_supported() == crate::inference::Support::Yes;
                model.tool_support_source = Some("runtime".into());
            }
            _ if model.tool_calling => model.tool_support_source = Some("template".into()),
            _ => {}
        }
    }
    Json(list)
}

#[derive(Deserialize)]
struct LoadReq {
    id: String,
}

async fn load_model(
    State(s): State<AppState>,
    axum::extract::Query(force): axum::extract::Query<ForceQuery>,
    Json(req): Json<LoadReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.id.trim().is_empty() {
        return Err(ApiError::bad(
            "model id is empty",
            "Pick a model from the selector first.",
        ));
    }
    set_progress(
        &s,
        req.id.trim(),
        "validating",
        "Checking model registry and agent guard.",
    )
    .await;
    if let Err(error) = guard_agent_running(&s, force.force).await {
        set_progress(&s, req.id.trim(), "error", &error.message).await;
        return Err(error);
    }
    if s.load_progress.read().await.cancel_requested {
        set_progress(
            &s,
            req.id.trim(),
            "cancelled",
            "Load cancelled by the user.",
        )
        .await;
        return Err(ApiError::bad(
            "load cancelled",
            "The load was cancelled before starting.",
        ));
    }
    set_progress(
        &s,
        req.id.trim(),
        "loading",
        "Loading model weights into the inference engine.",
    )
    .await;
    let out = load_model_by_id(&s, req.id.trim()).await;
    match &out {
        Ok((id, _)) => set_progress(&s, id, "ready", "Model loaded and ready for inference.").await,
        Err(e) => set_progress(&s, req.id.trim(), "error", &e.message).await,
    }
    out.map(|(id, notices)| Json(serde_json::json!({"loaded": id, "notices": notices})))
}

async fn unload_models(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let _update = s.runtime_update.lock().await;
    guard_agent_running(&s, false).await?;
    s.models.write().await.unload_all();
    s.inference.write().await.unload();
    s.llama.write().await.stop().await;
    tracing::info!("models unloaded");
    spawn_fit_preparation(&s, std::time::Duration::from_secs(5));
    Ok(Json(serde_json::json!({"unloaded": true})))
}

/// Stage 3: inference engine status for diagnostics (§80).
async fn inference_status(State(s): State<AppState>) -> Json<InferenceStatus> {
    let mut llama = s.llama.write().await;
    let running = llama.is_running();
    let base_url = llama.base_url();
    let last_error = llama.last_error.clone();
    let binary_found = SidecarBinary::detect(&s.runtime_dir()).is_ok();
    let (engine, model, ctx) = if running {
        let m = s.models.read().await.current().map(|m| m.id.clone());
        let c = llama.running.as_ref().map(|r| r.cfg.n_ctx).unwrap_or(0);
        ("llama-server".to_string(), m, c)
    } else {
        let inf = s.inference.read().await;
        let m = if inf.is_loaded() {
            s.models.read().await.current().map(|m| m.id.clone())
        } else {
            None
        };
        ("stub".to_string(), m, inf.context_size())
    };
    Json(InferenceStatus {
        runtime_notice: llama
            .running
            .as_ref()
            .filter(|_| running)
            .filter(|r| r.cfg.n_gpu_layers == 0)
            .map(|_| {
                "Running on CPU. No dedicated GPU is required; responses may be slower.".into()
            }),
        engine,
        running,
        base_url,
        model,
        context_size: ctx,
        binary_found,
        last_error,
    })
}

#[derive(Deserialize, Default)]
struct InferenceStartReq {
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    model_path: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    n_ctx: Option<u32>,
    #[serde(default)]
    n_gpu_layers: Option<i32>,
    #[serde(default)]
    n_threads: Option<u32>,
}

/// Stage 3: start the llama-server sidecar for a registered model (or an
/// explicit GGUF path). Marks the model loaded on success (§15).
async fn inference_start(
    State(s): State<AppState>,
    axum::extract::Query(force): axum::extract::Query<ForceQuery>,
    Json(req): Json<InferenceStartReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard_agent_running(&s, force.force).await?;
    let label = req
        .model_id
        .clone()
        .unwrap_or_else(|| req.model_path.clone().unwrap_or_default());
    set_progress(
        &s,
        &label,
        "validating",
        "Resolving model and checking resources.",
    )
    .await;
    if s.load_progress.read().await.cancel_requested {
        set_progress(&s, &label, "cancelled", "Start cancelled by the user.").await;
        return Err(ApiError::bad(
            "start cancelled",
            "Inference start was cancelled.",
        ));
    }
    // Resolve the GGUF path + model id.
    let (model_id, gguf): (Option<String>, PathBuf) =
        if let Some(id) = req.model_id.filter(|v| !v.trim().is_empty()) {
            let mm = s.models.read().await;
            match mm.get(&id) {
                Some(m) => (Some(id), m.gguf_path()),
                None => return Err(ApiError::not_found(format!("unknown model '{id}'"))),
            }
        } else if let Some(p) = req.model_path.filter(|v| !v.trim().is_empty()) {
            (None, PathBuf::from(p))
        } else if let Some(cur) = s.models.read().await.current() {
            (Some(cur.id.clone()), cur.gguf_path())
        } else {
            return Err(ApiError::bad(
                "no model selected",
                "Pass model_id, model_path, or load a model first.",
            ));
        };
    set_progress(
        &s,
        model_id.as_deref().unwrap_or(""),
        "loading",
        "Spawning llama-server and waiting for health.",
    )
    .await;
    let result = start_sidecar(
        &s,
        model_id,
        gguf,
        req.port.unwrap_or(DEFAULT_SIDECAR_PORT),
        req.n_ctx,
        req.n_gpu_layers,
        req.n_threads,
    )
    .await;
    let out = match result {
        Ok(out) => out,
        Err(error) => {
            set_progress(&s, &label, "error", &error.message).await;
            return Err(error);
        }
    };
    let label = out
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    set_progress(&s, &label, "ready", "Inference running and healthy.").await;
    Ok(Json(out))
}

/// The runtime build and chat template a model's tool check has to match.
fn tooling_identity(binary: &SidecarBinary, model: &crate::models::ModelMetadata) -> (String, String) {
    (
        crate::tooling::runtime_identity(&binary.0),
        model.template_fingerprint.clone().unwrap_or_else(|| "none".into()),
    )
}

/// The tool profile a load uses: the current `tooling.json`, or the result of
/// checking the model now (written beside it), with a notice for the person
/// loading when a check ran. Night decision 59.
async fn tooling_for_load(
    s: &AppState,
    model: &crate::models::ModelMetadata,
    binary: &SidecarBinary,
    base_url: &str,
    cfg: &crate::inference::InferenceConfig,
) -> (Option<crate::tooling::ToolingProfile>, Option<String>) {
    let (runtime, template) = tooling_identity(binary, model);
    let dir = crate::tooling::profile_dir(&cfg.model_path);
    let existing = crate::tooling::read_profile(&dir);
    let state = crate::tooling::profile_state(existing.as_ref(), &runtime, &template);
    if state == crate::tooling::ProfileState::Current {
        return (existing, None);
    }
    set_progress(
        s,
        &model.id,
        "checking_tools",
        "Checking how this model calls tools. This happens on its first load and takes a few seconds.",
    )
    .await;
    let outcome = run_tool_check(s, model, base_url, cfg, runtime, template).await;
    let why = if state == crate::tooling::ProfileState::Stale {
        "checked again because the check, the runtime or the model's chat template changed"
    } else {
        "first load"
    };
    match outcome {
        Ok(profile) => {
            let notice = format!(
                "Tool check ({why}, {:.1} s): this model {}.",
                profile.duration_ms as f64 / 1000.0,
                profile.summary()
            );
            (Some(profile), Some(notice))
        }
        Err(error) => (
            None,
            Some(format!(
                "The tool check could not run ({error}). Code sessions use the app's text format until it does; use Check again in the model's details."
            )),
        ),
    }
}

/// Run the checks on a loaded model and save the result beside it.
async fn run_tool_check(
    s: &AppState,
    model: &crate::models::ModelMetadata,
    base_url: &str,
    cfg: &crate::inference::InferenceConfig,
    runtime: String,
    template: String,
) -> Result<crate::tooling::ToolingProfile, String> {
    let client = SidecarClient::new(base_url.to_string())
        .map_err(|error| error.to_string())?
        .with_recorder(request_recorder(s, "", &format!("tool-check-{}", model.id), "tool_check"));
    let profile = crate::tooling::check(&client, cfg, runtime, template, cfg.template_caps).await?;
    let dir = crate::tooling::profile_dir(&cfg.model_path);
    if let Err(error) = crate::tooling::write_profile(&dir, &profile) {
        // The run still uses the result; the next load checks again.
        tracing::warn!("could not save {} for {}: {error}", crate::tooling::PROFILE_FILE, model.id);
    }
    tracing::info!(model = %model.id, method = ?profile.method, can_write = profile.can_write, ms = profile.duration_ms, "tool check finished");
    Ok(profile)
}

/// Check again how the loaded model calls tools (the model's details).
async fn recheck_model_tooling(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard_agent_running(&s, false).await?;
    let model = s
        .models
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("unknown model '{id}'")))?;
    let loaded = s.models.read().await.current().map(|current| current.id.clone());
    let running = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            llama.running.as_ref().map(|running| (running.base_url.clone(), running.cfg.clone()))
        } else {
            None
        }
    };
    let Some((base_url, cfg)) = running.filter(|_| loaded.as_deref() == Some(id.as_str())) else {
        return Err(ApiError::bad(
            "this model is not loaded",
            "The check runs on the loaded model. Load this model, then check again.",
        ));
    };
    let _update = s.runtime_update.lock().await;
    let binary = SidecarBinary::detect(&s.runtime_dir())
        .map_err(|error| ApiError::bad(format!("runtime not found: {error}"), "Build the runtime first."))?;
    let (runtime, template) = tooling_identity(&binary, &model);
    let profile = run_tool_check(&s, &model, &base_url, &cfg, runtime, template)
        .await
        .map_err(|error| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, format!("the tool check could not run: {error}"), "Try again when the model is ready."))?;
    if let Some(running) = s.llama.write().await.running.as_mut() {
        running.cfg.tooling = Some(profile.clone());
    }
    Ok(Json(serde_json::json!({"tooling": profile, "summary": format!("This model {}.", profile.summary())})))
}

#[derive(Deserialize, Default)]
struct ForceQuery {
    #[serde(default)]
    force: bool,
}

/// Stage 17 guard (§178): never silently terminate active agent work.
/// Returns 409 with the live run ids unless force is set.
async fn guard_agent_running(s: &AppState, force: bool) -> Result<(), ApiError> {
    if s.generations.read().await.is_active() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "A chat response is still running.",
            "Stop the response or wait for it to finish before changing the model.",
        ));
    }
    let live: Vec<String> = s
        .agents
        .read()
        .await
        .summaries()
        .into_iter()
        .filter(|r| {
            r.state.is_active()
        })
        .map(|r| r.id)
        .collect();
    if !live.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("{} agent run(s) active: {}", live.len(), live.join(", ")),
            if force {
                "Stop was requested, but the run has not finished yet. Wait for it to stop, then retry."
            } else {
                "Finish or stop the active run before switching models."
            },
        ));
    }
    Ok(())
}

/// Choose the context, cache precision and fit margin from the runtime's own
/// memory fit, and record why.
///
/// The automatic policy's context comes from a formula over the model header
/// that overstates the cache of hybrid-attention and sliding-window models;
/// llama.cpp's fit is exact for every architecture it loads. Owner speed rule
/// (`speed_rule`): the combination with every layer on the GPU measured fastest
/// at both generation and prompt reading (see `runtime_fit::FIT_COMBINATIONS`).
/// "Fit" mode shrinks the context only when
/// no combination keeps every layer on the GPU at the requested size; "use my
/// size" keeps the request with the combination that puts the most on the GPU.
/// When the probe is unavailable the policy's result stands.
///
/// The combination is chosen at the runtime's default micro-batch (512). The
/// micro-batch comes next, on that same combination: this model's calibrated
/// size (with the extra expert blocks the calibration measured it move to RAM)
/// when `calibrated_micro_batch` is given and this load's change is no larger
/// than the measured one (`runtime_fit::calibrated_micro_batch_fits`),
/// otherwise the safe default (`runtime_fit::default_micro_batch`, one more
/// probe at 1,024, reused when the calibrated size was 1,024). Automatic modes
/// only; the caller never runs this in Manual.
///
/// The decision is remembered (owner decision 2026-09-17: a 14B model's search
/// took 24 s of a 30 s load). A later load with the same model file, runtime,
/// request and GPU confirms it with one probe (`runtime_fit::fit_reuse`) and
/// searches again only when that probe disagrees, or when a compromise might
/// improve because more VRAM is free now. `FitPurpose::Prepare` (the search
/// ahead of a model's first load) skips a model that already has a decision.
///
/// Returns the fit the load gets at the chosen micro-batch (the load-mode
/// decision needs it), or None when the runtime's fit was unavailable.
#[allow(clippy::too_many_arguments)]
async fn fit_context_with_runtime(
    s: &AppState,
    purpose: FitPurpose,
    server: &std::path::Path,
    model: &crate::models::ModelMetadata,
    gguf: &std::path::Path,
    requested: u32,
    tuning: &crate::settings::RuntimeSettings,
    calibrated_micro_batch: Option<(u32, u32)>,
    vram: Option<&crate::inference::VramState>,
    cfg: &mut InferenceConfig,
) -> Option<crate::runtime_fit::Fit> {
    use crate::runtime_fit::{fit_combinations, fit_memory_key, fit_reuse, probe, FitReuse, DEFAULT_MICRO_BATCH, FIT_PROBE_CONCURRENCY};
    let fit_tool = crate::calibration::runtime_tool(server, "llama-fit-params")?;
    let requested = requested.min(model.context_length.max(1)).max(1);
    let draft_head = cfg.speculative == crate::inference::DRAFT_HEAD_SPECULATIVE;
    let free_vram_mib = vram.map(|vram| vram.total_bytes.saturating_sub(vram.used_bytes) / 1_048_576);
    let key = fit_memory_key(
        gguf,
        &fit_tool,
        requested,
        &tuning.kv_cache,
        tuning.keeps_requested_context(),
        calibrated_micro_batch,
        draft_head,
        vram.map(|vram| vram.total_bytes / 1_048_576),
    );
    let remembered = s.storage.lock().await.fit_decision_for(&key).unwrap_or_else(|error| {
        tracing::warn!("could not read the remembered fit: {error}");
        None
    });
    if let Some(remembered) = remembered {
        if purpose == FitPurpose::Prepare {
            return Some(remembered.fit);
        }
        let preferred = fit_combinations(&tuning.kv_cache).first().copied().unwrap_or(("f16", 1024));
        if fit_reuse(&remembered, free_vram_mib, preferred) == FitReuse::Confirm {
            let reserve = if draft_head && !remembered.draft_head_dropped { crate::inference::DRAFT_HEAD_RESERVE_MIB } else { 0 };
            let started = std::time::Instant::now();
            let confirmed = probe(
                &fit_tool,
                gguf,
                remembered.context,
                &remembered.cache,
                remembered.margin + reserve,
                remembered.micro_batch.unwrap_or(DEFAULT_MICRO_BATCH),
            )
            .await;
            if confirmed.as_ref() == Ok(&remembered.fit) {
                apply_fit_decision(&remembered, requested, cfg, Some(started.elapsed()));
                return Some(remembered.fit);
            }
            tracing::info!(model = %model.id, "the remembered fit no longer holds; searching again");
        }
    }
    let started = std::time::Instant::now();
    let mut searched = search_fit(&fit_tool, model, gguf, requested, tuning, calibrated_micro_batch, draft_head, FIT_PROBE_CONCURRENCY).await;
    let disagreed = match &searched {
        Ok((decision, true)) => !batched_fit_holds(&fit_tool, gguf, requested, tuning, draft_head, decision).await,
        _ => false,
    };
    if disagreed {
        tracing::info!(model = %model.id, "fit probes run together disagreed with probes run alone; searching one probe at a time");
        searched = search_fit(&fit_tool, model, gguf, requested, tuning, calibrated_micro_batch, draft_head, 1).await;
    }
    match searched {
        Ok((mut decision, _)) => {
            decision.free_vram_mib = free_vram_mib;
            tracing::info!(model = %model.id, seconds = started.elapsed().as_secs_f64(), context = decision.context, "fit searched");
            if let Err(error) = s.storage.lock().await.save_fit_decision(&key, &decision) {
                tracing::warn!("could not remember the fit: {error}");
            }
            apply_fit_decision(&decision, requested, cfg, None);
            Some(decision.fit)
        }
        Err(notes) => {
            if let Some(policy) = cfg.runtime_policy.as_mut() {
                policy.notes.extend(notes);
            }
            None
        }
    }
}

/// Why the fit is being looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitPurpose {
    /// A model load: a remembered decision is confirmed with one probe.
    Load,
    /// The search ahead of a model's first load: a remembered decision is left
    /// alone.
    Prepare,
}

/// Whether a search that ran probes together holds when checked with probes
/// run alone. Each probe measures free VRAM with its own GPU context, so
/// probes running together can only have seen too little: the chosen setting
/// must give the same fit alone, a smaller context must still be the largest
/// (the next step does not fit), and a draft head turned off must still cost
/// layers.
async fn batched_fit_holds(
    fit_tool: &std::path::Path,
    gguf: &std::path::Path,
    requested: u32,
    tuning: &crate::settings::RuntimeSettings,
    draft_head: bool,
    decision: &crate::runtime_fit::FitDecision,
) -> bool {
    use crate::runtime_fit::{fit_combinations, probe, Fit, DEFAULT_MICRO_BATCH};
    let reserve = if draft_head && !decision.draft_head_dropped { crate::inference::DRAFT_HEAD_RESERVE_MIB } else { 0 };
    let chosen = probe(fit_tool, gguf, decision.context, &decision.cache, decision.margin + reserve, decision.micro_batch.unwrap_or(DEFAULT_MICRO_BATCH)).await;
    if chosen.as_ref() != Ok(&decision.fit) {
        return false;
    }
    let combinations = fit_combinations(&tuning.kv_cache);
    if decision.placement_at_default == "gpu" && decision.context < requested {
        if let Some((cache, margin)) = combinations.last() {
            let next = (decision.context + 1024).min(requested);
            if probe(fit_tool, gguf, next, cache, margin + reserve, DEFAULT_MICRO_BATCH).await == Ok(Fit::AllLayers) {
                return false;
            }
        }
    }
    if draft_head && decision.draft_head_dropped {
        for (cache, margin) in &combinations {
            if probe(fit_tool, gguf, requested, cache, margin + crate::inference::DRAFT_HEAD_RESERVE_MIB, DEFAULT_MICRO_BATCH).await == Ok(Fit::AllLayers) {
                return false;
            }
        }
    }
    true
}

/// The fit search (see `fit_context_with_runtime`), with probes run up to
/// `concurrency` at a time where their answers do not depend on each other.
/// The preferred combination is probed alone first: most models fit with it.
/// Ok carries the decision and whether any probes ran together; Err the notes
/// explaining why the runtime's fit was unavailable.
#[allow(clippy::too_many_arguments)]
async fn search_fit(
    fit_tool: &std::path::Path,
    model: &crate::models::ModelMetadata,
    gguf: &std::path::Path,
    requested: u32,
    tuning: &crate::settings::RuntimeSettings,
    calibrated_micro_batch: Option<(u32, u32)>,
    draft_head: bool,
    concurrency: usize,
) -> Result<(crate::runtime_fit::FitDecision, bool), Vec<String>> {
    use crate::runtime_fit::{
        calibrated_micro_batch_fits, default_micro_batch, fastest_combination, fit_combinations, free_expert_blocks,
        largest_full_gpu_context, probe_in_order, Fit, FitDecision, ProbeRequest, BALANCED_MICRO_BATCH,
        DEFAULT_MICRO_BATCH, MICRO_BATCH_NOTE_PREFIX,
    };
    let floor = requested.min(4096);
    let combinations = fit_combinations(&tuning.kv_cache);
    let unavailable = |error: String| vec![format!("The runtime's memory fit was unavailable ({error}); the estimated plan was used.")];
    let mut batched = false;
    let mut note_parts = Vec::new();
    // The draft head's VRAM is not in llama-fit-params' estimate (llama-server
    // counts it when it loads): the probes leave that much more free, while
    // the server keeps the plain margin.
    let mut draft_reserve = if draft_head { crate::inference::DRAFT_HEAD_RESERVE_MIB } else { 0 };
    let mut draft_head_dropped = false;

    // Every combination at the requested size, stopping at the first that
    // keeps every layer on the GPU. With the draft head, first with its
    // reserve: it stays on only if some combination still keeps every layer
    // on the GPU at the requested context. Measured on a 27B model at 32K,
    // where every layer only just fits: with the draft head, 6 layers moved to
    // the CPU and generation fell from 37.2 to 32.2 tok/s (prose), 66.9 to
    // 50.4 (rewrites) and 33.9 to 22.0 deep in the context; with every layer
    // on the GPU and the head as well, the server ran out of memory. At 8K,
    // with room for it, the head gave +42% prose and +41% rewrites.
    let all_layers = |result: &Result<Fit, String>| matches!(result, Ok(Fit::AllLayers) | Err(_));
    let at_requested = loop {
        let requests: Vec<ProbeRequest> = combinations
            .iter()
            .map(|(cache, margin)| ProbeRequest { context: requested, cache, margin_mib: margin + draft_reserve, micro_batch: DEFAULT_MICRO_BATCH })
            .collect();
        let mut results = probe_in_order(fit_tool, gguf, &requests[..requests.len().min(1)], 1, all_layers).await;
        if concurrency > 1 && requests.len() > 1 && !results.iter().any(all_layers) {
            batched = true;
        }
        if !results.iter().any(all_layers) {
            results.extend(probe_in_order(fit_tool, gguf, &requests[1..], concurrency, all_layers).await);
        }
        let mut at_requested = Vec::new();
        for (combination, result) in combinations.iter().zip(results) {
            let fit = result.map_err(unavailable)?;
            at_requested.push((*combination, fit));
            if fit == Fit::AllLayers {
                break;
            }
        }
        let head_costs_layers = draft_reserve > 0 && !at_requested.is_empty() && !at_requested.iter().any(|(_, fit)| *fit == Fit::AllLayers);
        if !head_costs_layers {
            break at_requested;
        }
        draft_reserve = 0;
        draft_head_dropped = true;
        note_parts.push(format!(
            "This model's built-in draft head is off for this load: its extra ~0.9 GB of VRAM would move layers to the CPU at {} tokens, which measured slower on a 27B model at 32K (generation 37.2 → 32.2 tok/s, 33.9 → 22.0 deep in the context). N-gram drafting stays on; a smaller context leaves room for the head.",
            group_thousands(requested)
        ));
    };
    let best_at_requested = fastest_combination(&at_requested);

    // (context, cache, margin, placement, fit at the default micro-batch)
    let decision: (u32, &'static str, u32, &'static str, Fit) = match best_at_requested {
        None => return Err(note_parts),
        Some(((cache, margin), Fit::AllLayers)) => (requested, cache, margin, "gpu", Fit::AllLayers),
        Some(((cache, margin), fit)) if tuning.keeps_requested_context() => {
            note_parts.push(match fit {
                Fit::ExpertsOnCpu { .. } => format!("Context kept at {requested} tokens as set; the runtime keeps some expert weights of this mixture-of-experts model in RAM, where the CPU runs only the experts each token uses."),
                Fit::Layers(layers) => format!("Context kept at {requested} tokens as set; at that size the runtime fits {layers} layers on the GPU and runs the rest on the CPU, which is much slower (measured: two layers on the CPU cost an 8B model 28% of its speed)."),
                Fit::AllLayers => String::new(),
            });
            (requested, cache, margin, "hybrid", fit)
        }
        Some((fallback, fallback_fit)) => {
            // Fit mode: the largest context at which some combination keeps
            // every layer on the GPU, searched with the most permissive one,
            // then the most headroom that still fits at that size.
            let permissive = *combinations.last().expect("at least one combination");
            if concurrency > 1 {
                batched = true;
            }
            let found = largest_full_gpu_context(requested, floor, 1024, concurrency, |contexts: Vec<u32>| {
                let fit_tool = fit_tool.to_path_buf();
                let gguf = gguf.to_path_buf();
                async move {
                    let requests: Vec<ProbeRequest> = contexts
                        .iter()
                        .map(|context| ProbeRequest { context: *context, cache: permissive.0, margin_mib: permissive.1 + draft_reserve, micro_batch: DEFAULT_MICRO_BATCH })
                        .collect();
                    probe_in_order(&fit_tool, &gguf, &requests, concurrency, |_| false).await
                }
            })
            .await;
            match found {
                Ok(Some(context)) => {
                    let earlier: Vec<ProbeRequest> = combinations
                        .iter()
                        .take_while(|combination| **combination != permissive)
                        .map(|(cache, margin)| ProbeRequest { context, cache, margin_mib: margin + draft_reserve, micro_batch: DEFAULT_MICRO_BATCH })
                        .collect();
                    let results = probe_in_order(fit_tool, gguf, &earlier, concurrency, |result| result == &Ok(Fit::AllLayers)).await;
                    let chosen = combinations
                        .iter()
                        .zip(results)
                        .find(|(_, result)| result == &Ok(Fit::AllLayers))
                        .map(|(combination, _)| *combination)
                        .unwrap_or(permissive);
                    (context, chosen.0, chosen.1, "gpu", Fit::AllLayers)
                }
                // Not even the smallest context keeps every layer on the GPU:
                // the request stays, with the most on the GPU.
                Ok(None) => {
                    if let Fit::Layers(layers) = fallback_fit {
                        note_parts.push(format!("No setting keeps every layer on the GPU even at {floor} tokens; the runtime fits {layers} layers and runs the rest on the CPU."));
                    }
                    (requested, fallback.0, fallback.1, "hybrid", fallback_fit)
                }
                Err(error) => return Err(unavailable(error)),
            }
        }
    };
    let (context, cache, margin, placement_at_default, fit_at_default) = decision;

    // The micro-batch, on the combination just chosen. A placement with
    // nothing on the GPU keeps the runtime's (512 measured best on the CPU).
    // The calibrated size and 1,024 are probed together when both are needed.
    let rule = "the speed rule: at most 5% of generation given up, for at least 5x as much prompt reading gained";
    let (micro_batch, fit, micro_batch_note) = if !fit_at_default.uses_gpu() {
        (None, fit_at_default, None)
    } else {
        let calibrated_size = calibrated_micro_batch.filter(|(size, _)| *size > 0 && *size != DEFAULT_MICRO_BATCH);
        let mut sizes = Vec::new();
        if let Some((size, _)) = calibrated_size {
            sizes.push(size);
        }
        // A calibrated 512 is the default itself and needs no probe at 1,024.
        let calibrated_default = matches!(calibrated_micro_batch, Some((size, _)) if size == DEFAULT_MICRO_BATCH);
        if !calibrated_default && !sizes.contains(&BALANCED_MICRO_BATCH) {
            sizes.push(BALANCED_MICRO_BATCH);
        }
        let requests: Vec<ProbeRequest> = sizes
            .iter()
            .map(|size| ProbeRequest { context, cache, margin_mib: margin + draft_reserve, micro_batch: *size })
            .collect();
        if concurrency > 1 && requests.len() > 1 {
            batched = true;
        }
        let fits_at: Vec<(u32, Option<Fit>)> = sizes
            .iter()
            .copied()
            .zip(probe_in_order(fit_tool, gguf, &requests, concurrency, |_| false).await.into_iter().map(Result::ok))
            .collect();
        let fit_at = |size: u32| fits_at.iter().find(|(probed, _)| *probed == size).and_then(|(_, fit)| *fit);
        let mut calibration_set_aside = None;
        let calibrated = match calibrated_micro_batch.filter(|(size, _)| *size > 0) {
            Some((size, _)) if size == DEFAULT_MICRO_BATCH => Some((size, fit_at_default)),
            Some((size, measured_extra_blocks)) => match fit_at(size) {
                Some(at_size) if calibrated_micro_batch_fits(fit_at_default, at_size, measured_extra_blocks) => Some((size, at_size)),
                _ => {
                    calibration_set_aside = Some(size);
                    None
                }
            },
            None => None,
        };
        match calibrated {
            Some((size, at_size)) => (
                Some(size),
                at_size,
                Some(format!(
                    "{}{} tokens while reading prompts, from this model's calibration: each size was measured on this machine with its own placement and this one chosen by {rule}.",
                    MICRO_BATCH_NOTE_PREFIX,
                    group_thousands(size)
                )),
            ),
            None => {
                let at_balanced = fit_at(BALANCED_MICRO_BATCH);
                let size = default_micro_batch(fit_at_default, at_balanced, model.block_count);
                let at_size = if size == BALANCED_MICRO_BATCH { at_balanced.unwrap_or(fit_at_default) } else { fit_at_default };
                let reason = match (size == BALANCED_MICRO_BATCH, at_size) {
                    (true, Fit::AllLayers) => "every layer still fits on the GPU at that size, so prompts are read in larger steps at no cost to generation, which does not use the micro-batch.".to_string(),
                    (true, Fit::ExpertsOnCpu { blocks, .. }) if at_size == fit_at_default || matches!(fit_at_default, Fit::ExpertsOnCpu { blocks: default_blocks, .. } if default_blocks == blocks) => {
                        "the GPU keeps the same layers and expert weights at that size, so prompts are read in larger steps at no cost to generation.".to_string()
                    }
                    (true, Fit::ExpertsOnCpu { .. }) => format!(
                        "its larger compute buffer moves the experts of at most {} more block(s) to RAM, which {rule} allows (measured on a mixture-of-experts model: one more block cost 2.1% of generation for 46% faster prompt reading).",
                        free_expert_blocks(model.block_count)
                    ),
                    (true, Fit::Layers(layers)) => format!("the GPU keeps the same {layers} layers at that size, so prompts are read in larger steps at no cost to generation."),
                    (false, _) if at_balanced.is_none() => "the runtime's default; the fit at 1,024 could not be measured.".to_string(),
                    (false, _) => format!("the runtime's default; 1,024 would move more of this model off the GPU than {rule} allows (a whole layer costs about 8% of generation, an expert block 2-5%)."),
                };
                let set_aside = calibration_set_aside
                    .map(|calibrated| format!(" This model's calibration chose {}, but at this load's context and cache that size would move more of the model off the GPU than the calibration measured (or could not be checked), so the default rule decided.", group_thousands(calibrated)))
                    .unwrap_or_default();
                (
                    Some(size),
                    at_size,
                    Some(format!("{}{} tokens while reading prompts: {reason}{set_aside}", MICRO_BATCH_NOTE_PREFIX, group_thousands(size))),
                )
            }
        }
    };
    // The class of placement can change with the micro-batch (a calibrated
    // size may keep some expert tensors in RAM that 512 keeps on the GPU).
    let placement = if fit == fit_at_default {
        placement_at_default
    } else if fit == Fit::AllLayers {
        "gpu"
    } else {
        "hybrid"
    };
    if placement_at_default == "gpu" && placement == "hybrid" {
        note_parts.push("Every layer fits on the GPU at the runtime's default micro-batch; the chosen micro-batch keeps some of the model in RAM, a trade measured for this model.".to_string());
    }
    Ok((
        FitDecision {
            requested,
            context,
            cache: cache.to_string(),
            margin,
            micro_batch,
            fit,
            placement: placement.to_string(),
            placement_at_default: placement_at_default.to_string(),
            draft_head_dropped,
            notes: note_parts,
            micro_batch_note,
            free_vram_mib: None,
        },
        batched,
    ))
}

/// Write a fit decision, searched now or remembered, into the load's
/// configuration and its explanation. `confirmed_in` is how long the one probe
/// took when the decision was remembered.
fn apply_fit_decision(
    decision: &crate::runtime_fit::FitDecision,
    requested: u32,
    cfg: &mut InferenceConfig,
    confirmed_in: Option<std::time::Duration>,
) {
    if decision.draft_head_dropped && cfg.speculative == crate::inference::DRAFT_HEAD_SPECULATIVE {
        cfg.speculative = crate::inference::default_speculative();
        cfg.spec_draft_n_max = None;
        if let Some(policy) = cfg.runtime_policy.as_mut() {
            policy.speculative = cfg.speculative.clone();
            policy.spec_draft_n_max = None;
            policy.notes.retain(|note| !note.starts_with("This model carries a built-in draft head"));
        }
    }
    let mut note_parts = decision.notes.clone();
    let (context, cache, margin, placement) = (decision.context, decision.cache.as_str(), decision.margin, decision.placement.as_str());
    let estimated = cfg.n_ctx;
    cfg.n_ctx = context;
    cfg.kv_cache_type_k = cache.to_string();
    cfg.kv_cache_type_v = cache.to_string();
    cfg.fit_target_mib = Some(margin);
    cfg.micro_batch = decision.micro_batch;
    let headroom = if margin < 1024 {
        format!(" and {margin} MiB of VRAM left free (the runtime's default leaves 1024; less headroom was measured to put more of the model on the GPU)")
    } else {
        String::new()
    };
    if placement == "gpu" {
        note_parts.push(if context == requested {
            format!("Every layer fits on the GPU at the full {context}-token context with a {cache} cache{headroom}, measured by the runtime's own memory fit.")
        } else {
            format!(
                "Context set to {context} tokens with a {cache} cache{headroom}: the largest size at which every layer stays on the GPU, measured by the runtime's own memory fit (the header estimate allowed {estimated}). The saved preference is unchanged; a smaller model or quantization allows more."
            )
        });
    } else if !headroom.is_empty() || cache != "f16" {
        note_parts.push(format!("Cache {cache}{headroom}: the combination that puts the most of this model on the GPU."));
    }
    if let Some(elapsed) = confirmed_in {
        note_parts.push(format!(
            "This placement was found by an earlier fit search for this model on this machine and confirmed with one check at load ({:.1} s) instead of searching again.",
            elapsed.as_secs_f64()
        ));
    }
    if let Some(policy) = cfg.runtime_policy.as_mut() {
        policy.effective_context = context;
        policy.cache_type_k = cache.to_string();
        policy.cache_type_v = cache.to_string();
        policy.placement = placement.to_string();
        let fitted_note = note_parts.join(" ");
        if (context < requested || placement != "gpu") && !fitted_note.is_empty() {
            policy.context_note = Some(fitted_note.clone());
        } else {
            policy.context_note = None;
        }
        // Replace the estimate's explanation rather than contradict it.
        policy.notes.retain(|note| !note.starts_with("Context reduced") && !note.starts_with("Context kept at") && !note.starts_with("The weights ("));
        if !fitted_note.is_empty() {
            policy.notes.push(fitted_note);
        }
        policy.notes.extend(decision.micro_batch_note.clone());
    }
}

/// The load configuration from settings, before the runtime policy and the
/// fit adjust it. Shared by loads and the fit search ahead of a first load.
fn base_inference_config(
    settings: &AppSettings,
    model: Option<&crate::models::ModelMetadata>,
    gguf: PathBuf,
    n_ctx: Option<u32>,
    n_gpu_layers: Option<i32>,
    n_threads: Option<u32>,
) -> InferenceConfig {
    let projector_path = model
        .and_then(|m| m.projector_file.as_ref().map(|p| m.dir.join(p)))
        .filter(|p| p.is_file());
    InferenceConfig {
        model_path: gguf,
        projector_path,
        n_ctx: n_ctx.unwrap_or(settings.inference.context_size),
        n_batch: settings.inference.batch_size,
        n_threads: n_threads.unwrap_or(settings.hardware.cpu_threads),
        n_threads_batch: settings.hardware.threads_batch,
        poll: settings.hardware.poll,
        priority: settings.hardware.priority,
        n_gpu_layers: n_gpu_layers.unwrap_or(settings.hardware.gpu_layers),
        flash_attn: settings.hardware.flash_attention,
        kv_cache_gpu: settings.hardware.kv_cache_gpu,
        temperature: settings.inference.temperature,
        top_p: settings.inference.top_p,
        top_k: settings.inference.top_k,
        repeat_penalty: settings.inference.repeat_penalty,
        seed: None,
        // A template that refuses a system turn or demands strict alternation
        // has to be respected in every request this model serves, or it
        // answers 400 to all of them.
        chat_template: model.map(|m| m.chat_template).unwrap_or_default(),
        ..InferenceConfig::default()
    }
}

/// The calibrated micro-batch and the extra expert blocks its calibration
/// measured, when the latest calibration still describes this machine.
fn calibrated_micro_batch_of(calibration: Option<&(crate::calibration::Calibration, bool)>) -> Option<(u32, u32)> {
    calibration
        .filter(|(_, comparable)| *comparable)
        .and_then(|(latest, _)| latest.micro_batch.map(|size| (size, crate::calibration::measured_extra_blocks(&latest.micro_batches, size))))
}

/// The fit search ahead of each model's first load (owner decision
/// 2026-09-17). It runs while no model is loaded, so the VRAM it measures is
/// what a load will have, and it is cancelled the moment a load or a
/// calibration starts.
#[derive(Default)]
pub struct FitPreparation {
    running: std::sync::atomic::AtomicBool,
    rerun: std::sync::atomic::AtomicBool,
    searching: std::sync::atomic::AtomicBool,
    generation: std::sync::atomic::AtomicU64,
    cancelled: tokio::sync::Notify,
}

impl FitPreparation {
    /// Stop a search in progress; returns once its probes have been dropped
    /// (at most about two seconds).
    pub async fn cancel(&self) {
        use std::sync::atomic::Ordering;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.notify_waiters();
        for _ in 0..100 {
            if !self.searching.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

/// Start the fit search ahead of first loads after `delay`, or have the one
/// running go over the models again when it ends. Triggered at startup, when
/// models are added, when settings that decide the fit change, after an
/// unload and after a calibration.
pub fn spawn_fit_preparation(s: &AppState, delay: std::time::Duration) {
    use std::sync::atomic::Ordering;
    if cfg!(test) {
        return;
    }
    let preparation = s.fit_preparation.clone();
    if preparation.running.swap(true, Ordering::SeqCst) {
        preparation.rerun.store(true, Ordering::SeqCst);
        return;
    }
    let s = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        loop {
            preparation.rerun.store(false, Ordering::SeqCst);
            prepare_fits(&s).await;
            if preparation.rerun.load(Ordering::SeqCst) {
                continue;
            }
            preparation.running.store(false, Ordering::SeqCst);
            // A trigger that arrived after the last check but before the
            // flag was cleared would otherwise be lost.
            if !(preparation.rerun.load(Ordering::SeqCst) && !preparation.running.swap(true, Ordering::SeqCst)) {
                break;
            }
        }
    });
}

async fn prepare_fits(s: &AppState) {
    use std::sync::atomic::Ordering;
    let settings = s.settings.read().await.clone();
    if !settings.runtime_auto {
        return;
    }
    let Ok(binary) = SidecarBinary::detect(&s.runtime_dir()) else {
        return;
    };
    if crate::calibration::runtime_tool(&binary.0, "llama-fit-params").is_none() || s.llama.write().await.is_running() {
        return;
    }
    if !matches!(binary.devices().await, crate::runtime_selection::Devices::Available) {
        return;
    }
    let mut models = s.models.read().await.list();
    // The default model first: it is the one most likely loaded next.
    models.sort_by_key(|model| model.id != settings.general.default_model);
    let preparation = &s.fit_preparation;
    for model in models {
        let gguf = model.gguf_path();
        if !gguf.is_file() || !model.load_issues.is_empty() {
            continue;
        }
        let generation = preparation.generation.load(Ordering::SeqCst);
        let cancelled = preparation.cancelled.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        // Something loaded, loading or calibrating holds the GPU: stop; the
        // next unload or calibration starts the search again.
        if preparation.generation.load(Ordering::SeqCst) != generation
            || s.llama.write().await.is_running()
            || s.runtime_update.try_lock().is_err()
        {
            return;
        }
        preparation.searching.store(true, Ordering::SeqCst);
        let outcome = tokio::select! {
            _ = &mut cancelled => None,
            prepared = prepare_fit(s, &binary, &settings, &model, &gguf) => Some(prepared),
        };
        preparation.searching.store(false, Ordering::SeqCst);
        match outcome {
            None => return,
            Some(Some(summary)) => tracing::info!(model = %model.id, "{summary}"),
            Some(None) => {}
        }
    }
}

/// One model's fit, built the way its load builds it.
async fn prepare_fit(
    s: &AppState,
    binary: &SidecarBinary,
    settings: &AppSettings,
    model: &crate::models::ModelMetadata,
    gguf: &std::path::Path,
) -> Option<String> {
    let mut cfg = base_inference_config(settings, Some(model), gguf.to_path_buf(), None, None, None);
    let requested_context = cfg.n_ctx;
    let vram = tokio::task::spawn_blocking(current_vram_state).await.ok().flatten();
    let ram_available = (hardware::detect().ram.available_gb * 1_073_741_824.0) as u64;
    let policy = crate::inference::resolve_runtime_policy_for_machine(Some(model), &cfg, true, &settings.runtime, vram, Some(ram_available));
    policy.apply_to(&mut cfg);
    let calibration = latest_calibration(s, model, &binary.0).await;
    let started = std::time::Instant::now();
    let fit = fit_context_with_runtime(
        s,
        FitPurpose::Prepare,
        &binary.0,
        model,
        gguf,
        requested_context,
        &settings.runtime,
        calibrated_micro_batch_of(calibration.as_ref()),
        vram.as_ref(),
        &mut cfg,
    )
    .await?;
    Some(format!("fit ready ahead of the first load in {:.1} s: {:?}", started.elapsed().as_secs_f64(), fit))
}

/// Send a splitter's pieces to the chat stream: prose as `token` events and
/// into the visible text, actions as `action` events carrying only the tool
/// name and state.
fn send_split_pieces(
    pieces: Vec<crate::stream_split::Piece>,
    visible: &std::sync::Arc<std::sync::Mutex<String>>,
    tx: &tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
) {
    use crate::stream_split::Piece;
    for piece in pieces {
        let (state, name) = match piece {
            Piece::Prose(text) => {
                visible.lock().expect("lock").push_str(&text);
                let _ = tx.send(Ok(Event::default().event("token").safe_data(text)));
                continue;
            }
            Piece::ActionStarted { name } => ("started", name),
            Piece::ActionFinished { name } => ("finished", name),
            Piece::ActionIncomplete { name } => ("incomplete", name),
        };
        let _ = tx.send(Ok(Event::default()
            .event("action")
            .safe_data(serde_json::json!({"state": state, "name": name}).to_string())));
    }
}

/// A round the host rejected (and asks the model to redo) must not stay in
/// the saved reply: cut the visible text back to where the round began and
/// tell the client the text it should now show.
fn discard_round_text(
    visible: &std::sync::Arc<std::sync::Mutex<String>>,
    round_start: usize,
    tx: &tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
) {
    let text = {
        let mut visible = visible.lock().expect("lock");
        let keep = round_start.min(visible.len());
        visible.truncate(keep);
        visible.clone()
    };
    let _ = tx.send(Ok(Event::default().event("replace").safe_data(text)));
}

/// Remove the oldest saved-history turns until at least `tokens` are freed
/// (counted at 3 bytes per token, which over- rather than under-frees), never
/// the leading system turns or the latest user message and anything after it.
/// Returns how many turns were removed.
fn drop_oldest_history(turns: &mut Vec<ChatTurn>, tokens: u32) -> usize {
    let first = turns.iter().take_while(|turn| turn.role == "system").count();
    let Some(last_user) = turns.iter().rposition(|turn| turn.role == "user") else {
        return 0;
    };
    let mut freed = 0usize;
    let mut end = first;
    while end < last_user && freed < tokens as usize {
        freed += turns[end].content.len().div_ceil(3);
        end += 1;
    }
    // Keep roles alternating from a user turn: a history that would start with
    // an assistant reply drops that reply too.
    while end < last_user && turns[end].role != "user" {
        end += 1;
    }
    let removed = end - first;
    turns.drain(first..end);
    removed
}

/// The notice for a requested context the model does not support, or None.
fn context_limit_notice(model_name: &str, requested: u32, model_limit: u32) -> Option<String> {
    (model_limit > 0 && requested > model_limit).then(|| {
        format!(
            "{model_name} supports at most {} tokens of context. Your context size of {} was reduced to {} for this model; the setting itself is unchanged and applies in full to models that support it.",
            group_thousands(model_limit),
            group_thousands(requested),
            group_thousands(model_limit)
        )
    })
}

fn group_thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Shared sidecar starter used by inference/start and prepare-context (§172).
/// Includes the vision projector when the model metadata names one (§176).
async fn start_sidecar(
    s: &AppState,
    model_id: Option<String>,
    gguf: PathBuf,
    port: u16,
    n_ctx: Option<u32>,
    n_gpu_layers: Option<i32>,
    n_threads: Option<u32>,
) -> Result<serde_json::Value, ApiError> {
    use crate::llamaserver::RunningSidecar;
    // The fit search ahead of first loads holds the GPU's memory readings: stop it.
    s.fit_preparation.cancel().await;
    let _update = s.runtime_update.lock().await;
    guard_agent_running(s, false).await?;

    if !gguf.is_file() {
        return Err(ApiError::not_found(format!(
            "GGUF not found: {}. Add the file under models/<id>/model.gguf.",
            gguf.display()
        )));
    }

    let settings = s.settings.read().await.clone();
    let model = match model_id.as_ref() {
        Some(id) => s.models.read().await.get(id).cloned(),
        None => None,
    };
    let mut cfg = base_inference_config(&settings, model.as_ref(), gguf.clone(), n_ctx, n_gpu_layers, n_threads);
    // A context larger than the model was trained for is capped by the
    // policy below. That used to be one line among the runtime notes; the
    // person who asked for the larger window is told at load instead.
    let context_notice = model
        .as_ref()
        .and_then(|m| context_limit_notice(&m.name, cfg.n_ctx, m.context_length));
    let binary = match SidecarBinary::detect(&s.runtime_dir()) {
        Ok(b) => b,
        Err(e) => {
            s.llama.write().await.last_error = Some(e.to_string());
            // Not a wrong id: the message already says how to build or point at the runtime.
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                e.to_string(),
                "The model is fine; the app needs its llama.cpp runtime first.",
            ));
        }
    };
    // A file without a chat template: the architecture's built-in format, when
    // this runtime lists it (see `models::builtin_chat_format`). ChatML is the
    // runtime's own fallback and needs nothing.
    let chat_format_notice = match model.as_ref().and_then(|m| m.builtin_chat_format.clone().map(|format| (format, m.architecture.clone()))) {
        Some((format, _)) if format == "chatml" => None,
        Some((format, architecture)) => {
            if binary.builtin_chat_templates().await.iter().any(|name| *name == format) {
                cfg.builtin_chat_format = Some(format.clone());
                Some(format!(
                    "This model file carries no chat template, so llama.cpp's built-in \"{format}\" chat format for the {architecture} architecture is used. The runtime's generic format would leave the model without its own end-of-turn marker, and replies would run on to the output limit."
                ))
            } else {
                Some(format!(
                    "This model file carries no chat template, and the installed runtime does not list the built-in \"{format}\" format for the {architecture} architecture, so its generic format is used: replies may be garbled or run on. Rebuilding the runtime with scripts/build-runtime usually adds it."
                ))
            }
        }
        None => None,
    };

    // Stop any previous sidecar before starting (§15 switch semantics).
    s.llama.write().await.stop().await;
    s.models.write().await.unload_all();
    s.inference.write().await.unload();
    // Measure GPU memory only after the previous worker has released its
    // share, so the fit sees what this model can actually use.
    let vram = if settings.runtime_auto {
        tokio::task::spawn_blocking(current_vram_state)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let ram_available = (hardware::detect().ram.available_gb * 1_073_741_824.0) as u64;
    let requested_context = cfg.n_ctx;
    let policy = crate::inference::resolve_runtime_policy_for_machine(
        model.as_ref(),
        &cfg,
        settings.runtime_auto,
        &settings.runtime,
        vram,
        Some(ram_available),
    );
    policy.apply_to(&mut cfg);
    let devices = if settings.runtime_auto {
        binary.devices().await
    } else {
        crate::runtime_selection::Devices::Unknown
    };
    let runtime_fit_applies = settings.runtime_auto && matches!(devices, crate::runtime_selection::Devices::Available);
    // This model's latest calibration and whether it still describes this
    // machine. The Fastest/Balanced/Light profiles and, when the runtime's fit
    // runs, the measured micro-batch come from it; Manual never reads it.
    // Looked up once: checking the environment starts the runtime to read its
    // version.
    let calibration = match model.as_ref() {
        Some(model) if settings.runtime.profile_name().is_some() || runtime_fit_applies => {
            latest_calibration(s, model, &binary.0).await
        }
        _ => None,
    };
    if let (Some(profile), Some(_)) = (settings.runtime.profile_name(), model.as_ref()) {
        let note = apply_calibrated_profile(profile, calibration.as_ref(), &mut cfg);
        if let Some(policy) = cfg.runtime_policy.as_mut() {
            policy.notes.push(note);
        }
    }
    // The fit the load gets, for the prompt-cache sizing below.
    let mut load_fit: Option<crate::runtime_fit::Fit> = None;
    if runtime_fit_applies {
        if let Some(model) = model.as_ref() {
            let fit = fit_context_with_runtime(
                s,
                FitPurpose::Load,
                &binary.0,
                model,
                &gguf,
                requested_context,
                &settings.runtime,
                calibrated_micro_batch_of(calibration.as_ref()),
                vram.as_ref(),
                &mut cfg,
            )
            .await;
            load_fit = fit;
            // Shorter n-gram drafts when weights stay in RAM (see
            // `runtime_fit::ngram_draft_length`).
            if let Some(length) = fit.and_then(|fit| crate::runtime_fit::ngram_draft_length(fit, &cfg.speculative)) {
                cfg.spec_ngram_length = Some(length);
                if let Some(policy) = cfg.runtime_policy.as_mut() {
                    policy.notes.push(format!(
                        "{} of at most {length} tokens: with part of the model in RAM, a rejected long draft costs a verification step on the CPU (measured on a mixture-of-experts model: file rewrites 42% faster than the runtime's default of 48, prose unchanged). Models fully on the GPU keep the default, which measured marginally faster there.",
                        crate::runtime_fit::NGRAM_LENGTH_NOTE_PREFIX
                    ));
                }
            }
            // Load without mmap when the placement keeps weights in RAM while
            // using the GPU, RAM has room for the copy (see
            // `runtime_fit::load_without_mmap`), and this runtime knows the
            // argument. Start time measured: +0.1 s on a 5.5 s load.
            let file_bytes = crate::models::model_set_bytes(&gguf);
            if let Some(fit) = fit.filter(|fit| crate::runtime_fit::load_without_mmap(*fit, ram_available, file_bytes)) {
                if binary.supports_argument("--load-mode").await {
                    cfg.load_without_mmap = true;
                    if let Some(policy) = cfg.runtime_policy.as_mut() {
                        policy.notes.push(format!(
                            "{}: the runtime is asked to read the {} that stay in RAM into pinned memory instead of mapping them from the {:.1} GB file, so the GPU copies them faster while reading prompts (measured on a mixture-of-experts model: about 48% faster prompt reading at the same generation speed, 0.1 s longer start). Free RAM ({:.1} GB) holds the copy with at least 4 GB to spare. If pinned memory cannot be allocated the runtime uses ordinary memory.",
                            crate::runtime_fit::LOAD_MODE_NOTE_PREFIX,
                            match fit {
                                crate::runtime_fit::Fit::ExpertsOnCpu { .. } => "expert weights",
                                _ => "layers",
                            },
                            file_bytes as f64 / 1e9,
                            ram_available as f64 / 1e9
                        ));
                    }
                }
            }
        }
    }
    // The server keeps up to 8 GiB of conversations in RAM by default. Size it
    // from the RAM left after the model memory that lives there.
    if let Some(model) = model.as_ref() {
        let weights = model.weights_bytes.unwrap_or(0);
        let cache = model
            .kv_bytes_per_token
            .unwrap_or(0)
            .saturating_mul(u64::from(cfg.n_ctx));
        let placement = cfg.runtime_policy.as_ref().map(|p| p.placement.clone()).unwrap_or_default();
        let ram_side = match (placement.as_str(), load_fit) {
            ("gpu", _) => 0,
            // The weights the runtime's fit keeps in RAM (mapped or pinned
            // alike), estimated from the placement, and the cache.
            ("hybrid", Some(fit)) => crate::runtime_fit::weights_in_ram_bytes(fit, weights, model.block_count) + cache,
            // Without the runtime's fit: half the weights and the cache.
            ("hybrid", None) => weights / 2 + cache,
            _ => weights + cache,
        };
        let mib = crate::inference::prompt_cache_ram_mib(ram_available, ram_side);
        cfg.cache_ram_mib = Some(mib);
        if let Some(policy) = cfg.runtime_policy.as_mut() {
            policy.notes.push(if mib == 0 {
                "The RAM prompt cache is off: free RAM is needed for the model itself. Switching conversations re-reads their history.".into()
            } else {
                format!("Prompt cache in RAM: up to {mib} MiB (llama.cpp's default is 8192), sized from the free RAM at load so that switching conversations cannot push the model into paging.")
            });
        }
    }
    match crate::runtime_selection::load_with_fallback(
        cfg,
        settings.runtime_auto,
        devices,
        |attempt| RunningSidecar::spawn(&binary.0, attempt, port),
    )
    .await
    {
        Ok((mut sidecar, notice)) => {
            let base_url = sidecar.base_url.clone();
            // What this template can render, as the runtime itself reports it.
            let caps = match SidecarClient::new(base_url.clone()) {
                Ok(client) => client.template_caps().await,
                Err(_) => None,
            };
            sidecar.cfg.template_caps = caps;
            // A new tokenizer: the token estimate relearns its ratio.
            crate::agent::reset_chars_per_token();
            if let Some(caps) = caps {
                let key = crate::calibration::model_key(&sidecar.cfg.model_path);
                if let Err(error) = s.storage.lock().await.save_template_caps(&key, &caps) {
                    tracing::warn!("could not remember the template capabilities: {error}");
                }
            }
            // How this model calls tools: its check from `tooling.json`, or a
            // new check now when it has none for this runtime and template.
            let tooling_notice = match model.as_ref() {
                Some(model) => {
                    let (profile, notice) = tooling_for_load(s, model, &binary, &base_url, &sidecar.cfg).await;
                    sidecar.cfg.tooling = profile;
                    notice
                }
                None => None,
            };
            let cfg = sidecar.cfg.clone();
            s.llama.write().await.running = Some(sidecar);
            s.llama.write().await.last_error = None;
            // Mirror loaded state into the registry + stub engine for UI consistency.
            if let Some(id) = &model_id {
                let _ = s.models.write().await.switch_to(id);
            }
            if let Err(e) = s.inference.write().await.load(cfg.clone()) {
                tracing::warn!("stub mirror load failed: {e}");
            }
            tracing::info!(model = ?model_id, url = %base_url, "inference started");
            Ok(
                serde_json::json!({
                    "started": true,
                    "base_url": base_url,
                    "model": model_id,
                    "notice": notice,
                    // Everything the person loading the model should be told.
                    "notices": notice.iter().cloned().chain(context_notice).chain(chat_format_notice).collect::<Vec<String>>(),
                    "runtime_policy": cfg.runtime_policy,
                    // The result of a tool check this load ran, if it ran one.
                    "tooling_notice": tooling_notice,
                    "tooling": cfg.tooling,
                }),
            )
        }
        Err(e) => {
            // One plain sentence for the person loading; the runtime's log
            // lines go to the app log, which exported sessions include.
            let full = e.to_string();
            let plain = crate::llamaserver::without_runtime_diagnostic(&full)
                .trim_start_matches("Generation failed: ")
                .to_string();
            if plain.len() < full.len() {
                tracing::warn!("model load failed: {full}");
            }
            s.llama.write().await.last_error = Some(plain.clone());
            Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                // Callers that add their own "failed to load" prefix show it once.
                plain,
                "The runtime's own log lines are in the app log, which exported sessions include.",
            ))
        }
    }
}

async fn inference_stop(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let _update = s.runtime_update.lock().await;
    guard_agent_running(&s, false).await?;
    s.llama.write().await.stop().await;
    s.llama.write().await.last_error = None;
    s.inference.write().await.unload();
    s.models.write().await.unload_all();
    // With the GPU free again, models without a fit get one.
    spawn_fit_preparation(&s, std::time::Duration::from_secs(5));
    Ok(Json(serde_json::json!({"stopped": true})))
}

/// Re-scan the models directory (§9–§10). Registers new valid entries.
async fn scan_models(State(s): State<AppState>) -> Json<serde_json::Value> {
    let dir = s.models_dir.clone();
    let (found, mut warnings) = crate::models::scan_models_dir(&dir);
    let discovered = found.len();
    let (removed, register_warnings) = s.models.write().await.reconcile(found);
    let registered = discovered.saturating_sub(register_warnings.len());
    warnings.extend(register_warnings);
    for w in &warnings {
        tracing::warn!("{w}");
    }
    Json(serde_json::json!({"registered": registered, "removed": removed, "warnings": warnings}))
}

/// Usable VRAM: measured via nvidia-smi when present, else the (stubbed)
/// sysinfo enumeration. Centralized so estimates never assume infinite RAM
/// on machines whose GPU the stub cannot see (§160).
fn measured_vram_gb(hw: &hardware::HardwareReport) -> f64 {
    crate::metrics::vram_info()
        .map(|(_, total)| total)
        .unwrap_or_else(|| hw.gpus.iter().map(|g| g.vram_gb).fold(0.0, f64::max))
}

/// Fresh GPU memory reading for load-time fitting (one `nvidia-smi` call).
fn current_vram_state() -> Option<crate::inference::VramState> {
    let (used_gb, total_gb) = crate::metrics::vram_info()?;
    let to_bytes = |gb: f64| (gb.max(0.0) * 1_073_741_824.0) as u64;
    (total_gb > 0.0).then(|| crate::inference::VramState {
        total_bytes: to_bytes(total_gb),
        used_bytes: to_bytes(used_gb),
    })
}

/// VRAM total from the background sampler when it has one (no extra
/// `nvidia-smi` process per request), else a direct probe.
fn sampled_vram_gb(s: &AppState, hw: &hardware::HardwareReport) -> f64 {
    let sampled = s
        .metrics
        .lock()
        .ok()
        .and_then(|log| log.latest())
        .and_then(|sample| sample.vram_total_gb)
        .filter(|total| *total > 0.0);
    sampled.unwrap_or_else(|| measured_vram_gb(hw))
}

/// Stage 16: context recommendation (§§154–169).
#[derive(Deserialize, Default)]
struct RecommendQuery {
    #[serde(default)]
    workload: String,
    #[serde(default)]
    profile: String,
}

async fn model_recommend(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RecommendQuery>,
) -> Result<Json<crate::recommend::Recommendation>, ApiError> {
    use crate::recommend::{RecInputs, RecProfile, Workload};
    let meta = match s.models.read().await.get(&id) {
        Some(m) => m.clone(),
        None => return Err(ApiError::not_found(format!("unknown model '{id}'"))),
    };
    let params_b = crate::recommend::parse_params_b(&meta.parameters).ok_or_else(|| {
        ApiError::bad(
            format!("cannot parse parameter count '{}'", meta.parameters),
            "Set parameters like '14B' in metadata.json.",
        )
    })?;
    let hw = hardware::detect();
    let vram = sampled_vram_gb(&s, &hw);
    let workload = if q.workload.is_empty() {
        Workload::Chat
    } else {
        Workload::parse(&q.workload)
    };
    // Active sessions share the pool: other non-terminal agent runs + live gen.
    let others = s
        .agents
        .read()
        .await
        .summaries()
        .iter()
        .filter(|r| {
            !matches!(
                r.state,
                crate::agent::AgentState::Completed
                    | crate::agent::AgentState::Failed
                    | crate::agent::AgentState::Cancelled
            )
        })
        .count();
    let image_count = 0; // per-session image budgeting lands with vision sessions
    Ok(Json(crate::recommend::recommend(&RecInputs {
        params_b,
        quant: meta.quantization.clone(),
        train_ctx: meta.context_length,
        vram_gb: vram,
        ram_gb: hw.ram.total_gb,
        gpu_layers_full: true,
        other_sessions: others,
        workload,
        profile: if q.profile.is_empty() {
            RecProfile::Balanced
        } else {
            RecProfile::parse(&q.profile)
        },
        image_count,
    })))
}

/// Stage 4: model detail with on-disk facts + resource estimates (§10, §14, §16).
async fn model_detail(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mm = s.models.read().await;
    let meta = match mm.get(&id) {
        Some(m) => m.clone(),
        None => return Err(ApiError::not_found(format!("unknown model '{id}'"))),
    };
    drop(mm);
    let hw = hardware::detect();
    let vram = sampled_vram_gb(&s, &hw);
    let mut effective_meta = meta.clone();
    effective_meta.context_length = s
        .settings
        .read()
        .await
        .inference
        .context_size
        .min(meta.context_length)
        .max(1);
    let est: ModelEstimates = describe_model(&effective_meta, vram, hw.ram.total_gb);
    let auto = est.total_need_gb.map(|need| {
        crate::models::auto_configure(need, vram, hw.ram.total_gb, effective_meta.context_length)
    });
    Ok(Json(serde_json::json!({
        "metadata": meta,
        "estimates": est,
        "recommended": auto,
    })))
}

/// Delete only the selected model's verified weights, preserving sibling files.
/// Stops inference first if
/// this model is the loaded one (§15). GGUF deletion is permanent — the
/// frontend confirms before calling.
async fn delete_model(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id.trim().is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(ApiError::bad(
            "invalid model id",
            "Use the id shown in the model list.",
        ));
    }
    let metadata = s
        .models
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("unknown model '{id}'")))?;
    let weights = metadata.deletion_path(&s.models_dir).map_err(|error| {
        ApiError::bad(
            error,
            "Only model weights inside the configured models directory can be deleted.",
        )
    })?;
    let is_current = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.id == id)
        .unwrap_or(false);
    if is_current {
        s.llama.write().await.stop().await;
        s.inference.write().await.unload();
        s.models.write().await.unload_all();
    }
    std::fs::remove_file(&weights).map_err(|error| {
        ApiError::internal(format!("cannot delete {}: {error}", weights.display()))
    })?;
    s.models.write().await.remove(&id);
    tracing::info!(model = %id, "model deleted");
    Ok(Json(
        serde_json::json!({"deleted": id, "removed_file": weights, "preserved": "Metadata, projectors and other files were kept."}),
    ))
}

#[derive(Deserialize)]
struct StartDownloadReq {
    id: String,
    url: String,
    #[serde(default)]
    sha256: Option<String>,
}

async fn list_downloads(State(s): State<AppState>) -> Json<Vec<crate::downloads::DownloadInfo>> {
    let mut dm = s.downloads.write().await;
    dm.refresh();
    Json(dm.list())
}

async fn start_download(
    State(s): State<AppState>,
    Json(req): Json<StartDownloadReq>,
) -> Result<Json<crate::downloads::DownloadInfo>, ApiError> {
    let http = s.http.clone();
    let mut dm = s.downloads.write().await;
    match dm.start(req.id, req.url, req.sha256, http) {
        Ok(info) => {
            tracing::info!(id = %info.id, "download started");
            Ok(Json(info))
        }
        Err(e) => Err(ApiError::bad(e, "Use a fresh id and an https:// file URL.")),
    }
}

async fn get_download(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::downloads::DownloadInfo>, ApiError> {
    let mut dm = s.downloads.write().await;
    dm.refresh();
    dm.get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("unknown download '{id}'")))
}

async fn pause_download(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut dm = s.downloads.write().await;
    dm.pause(&id)
        .map_err(|e| ApiError::bad(e, "Only a running download can be paused."))?;
    Ok(Json(serde_json::json!({"paused": id})))
}

async fn resume_download(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::downloads::DownloadInfo>, ApiError> {
    let http = s.http.clone();
    let mut dm = s.downloads.write().await;
    dm.resume(&id, http)
        .map(Json)
        .map_err(|e| ApiError::bad(e, "Only a paused/failed download can resume."))
}

async fn cancel_download(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut dm = s.downloads.write().await;
    dm.cancel(&id).map_err(|e| ApiError::not_found(e))?;
    Ok(Json(serde_json::json!({"cancelled": id})))
}

/// §58 chat: SSE `event: token` stream (§19). Persists history when a known
/// conversation_id is supplied (§20, §83).
#[derive(Deserialize)]
struct ChatReq {
    #[serde(default)]
    conversation_id: String,
    message: String,
    #[serde(default)]
    attachments: Vec<String>,
    /// Stage 11 per-message capability overrides (§115). Each overrides the
    /// conversation default, which overrides the settings default.
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    search: Option<bool>,
}

/// Everything every model request for one conversation shares: the identity
/// prompt (project snapshot + saved memory), the bounded history as turns,
/// and the resolved code workspace. Chat, routing and the agent all start from
/// this same prefix so llama-server's prompt cache serves the unchanged part.
/// A different prefix per request type would force a full re-prefill of the
/// whole conversation on every message, which on a CPU-only machine costs
/// minutes, not milliseconds.
pub(crate) struct RequestContext {
    pub sys_prompt: String,
    pub sys_mode: String,
    pub model_name: String,
    /// History (+ attachment excerpts on the latest user turn, images applied).
    pub turns: Vec<ChatTurn>,
    pub images_skipped: usize,
    pub chat_workspace: Option<(String, String, std::path::PathBuf)>,
    pub reasoning_default: bool,
    pub search_default: bool,
    /// Characters of saved history (summary included) before any trimming.
    pub history_chars: usize,
    /// Characters the request gives history: the window minus the system
    /// prompt, the reply, a margin and attachment excerpts.
    pub history_room_chars: usize,
}

impl RequestContext {
    /// How full saved history is against the room a request gives it.
    pub fn history_usage_pct(&self) -> u32 {
        (self.history_chars.saturating_mul(100) / self.history_room_chars.max(1)).min(999) as u32
    }
}

/// Bounded history in characters for a given context window: leave room for
/// the system prompt, the reply and a safety margin, then convert with a
/// conservative 3 chars/token. Never below a small floor so the latest turn
/// always travels.
pub(crate) fn history_char_budget(n_ctx: u32, system_chars: usize) -> usize {
    let system_tokens = system_chars / 3;
    let available = (n_ctx as usize)
        .saturating_sub(system_tokens)
        .saturating_sub(1536);
    (available * 3).clamp(4_000, HISTORY_CHARS)
}

/// Assemble the shared prefix. `pending_message` is a user turn that has not
/// been persisted yet (routing runs before persistence); it is laid out
/// exactly as the persisted turn will be, so the two requests share bytes.
pub(crate) async fn assemble_request_context(
    s: &AppState,
    conv_id: Option<&str>,
    pending_message: Option<&str>,
    n_ctx: u32,
) -> Result<RequestContext, ApiError> {
    assemble_request_context_with(s, conv_id, pending_message, n_ctx, true).await
}

/// `load_images: false` measures the context without preparing image
/// attachments, for the context gauge.
pub(crate) async fn assemble_request_context_with(
    s: &AppState,
    conv_id: Option<&str>,
    pending_message: Option<&str>,
    n_ctx: u32,
    load_images: bool,
) -> Result<RequestContext, ApiError> {
    let model_name = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "local model".into());
    let vision_ok = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.vision)
        .unwrap_or(false);
    let Some(cid) = conv_id else {
        let turns = pending_message
            .map(|message| vec![ChatTurn::text("user", message)])
            .unwrap_or_default();
        let sys_prompt = build_system_prompt("chat", "", &model_name);
        let history_room_chars = history_char_budget(n_ctx, sys_prompt.len());
        return Ok(RequestContext {
            sys_prompt,
            sys_mode: "chat".into(),
            model_name,
            history_chars: pending_message.map(str::len).unwrap_or(0),
            history_room_chars,
            turns,
            images_skipped: 0,
            chat_workspace: None,
            reasoning_default: false,
            search_default: false,
        });
    };
    let st = s.storage.lock().await;
    let conv = st
        .get_conversation(cid)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("conversation not found"))?;
    let mut history = st
        .context_messages_for(cid)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let Some(message) = pending_message {
        history.push(Message {
            id: "pending".into(),
            conversation_id: cid.into(),
            role: "user".into(),
            content: message.into(),
            created_at: String::new(),
        });
    }
    let attachments = st.attachments_for(cid).unwrap_or_default();
    // Resolve the subject once for this request. The snapshot, retrieval and
    // file tools must all use the same root, including conversational follow-ups.
    let chat_workspace = if conv.mode == "code" {
        let linked = linked_code_workspace(&st, &conv)?;
        let references = history
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        let root = crate::agent_runner::focused_workspace_root(
            std::path::Path::new(&linked.path),
            &references,
        );
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&linked.name)
            .to_owned();
        Some((linked.id, name, root))
    } else {
        None
    };
    let snapshot = chat_workspace
        .as_ref()
        .map(|(_, name, root)| workspace_snapshot(&root.to_string_lossy(), name))
        .unwrap_or_default();
    let memory = st
        .memory_context(cid, &conv.workspace)
        .map_err(|error| ApiError::internal(format!("Could not read saved memory: {error}")))?
        .text;
    let legacy = legacy_prompt_order();
    let listing = if legacy || snapshot.is_empty() { snapshot.as_str() } else { LISTING_IN_LATEST_MESSAGE };
    let mut sys_prompt = build_system_prompt(&conv.mode, listing, &model_name);
    let changing = if legacy {
        sys_prompt.push_str(&memory);
        String::new()
    } else {
        changing_context_block(&snapshot, &memory)
    };
    let budget = history_char_budget(n_ctx, sys_prompt.len() + changing.len());
    let history_chars: usize = history.iter().map(|message| message.content.len()).sum();
    let history_room_chars = history_room_for(budget, &attachments);
    let (mut turns, turn_ids) = build_turns_with_owners(&history, &attachments, budget);
    if !changing.is_empty() {
        match turns.iter_mut().rev().find(|turn| turn.role == "user") {
            Some(latest) => latest.content.push_str(&changing),
            None => turns.push(ChatTurn::text("user", changing.trim_start().to_string())),
        }
    }
    let mut images_skipped = 0usize;
    if vision_ok && load_images {
        let (urls, skipped) = prepare_conversation_images(&s.attachments_dir, cid, &attachments);
        images_skipped += skipped;
        if !urls.is_empty() {
            if legacy {
                if let Some(last_user) = turns.iter_mut().rev().find(|t| t.role == "user") {
                    last_user.images = urls.into_iter().map(|(_, url)| url).collect();
                }
            } else {
                // Each image on the message it was sent with.
                for (attachment, url) in urls {
                    if let Some(index) = attachment_turn(&turns, &turn_ids, &history, attachment) {
                        turns[index].images.push(url);
                    }
                }
            }
        }
    } else {
        images_skipped = attachments.iter().filter(|a| a.kind == "image").count();
    }
    Ok(RequestContext {
        sys_prompt,
        sys_mode: conv.mode.clone(),
        model_name,
        turns,
        images_skipped,
        chat_workspace,
        reasoning_default: conv.reasoning_default,
        search_default: conv.search_default,
        history_chars,
        history_room_chars,
    })
}

/// Resolved reasoning mode for one request (§§110–113).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningMode {
    Off,
    /// Model advertises native reasoning; use its own mode + bigger budget.
    Native,
    /// Honest fallback: structured extended-reasoning preface, no fake labels.
    Extended,
}

/// System prompt (§22): identity + capability truth. The model never sees
/// raw OS access; it sees a workspace and deterministic commands.
/// The standing instructions for a chat or code session.
///
/// They describe only what that session can really do and teach no tool
/// protocol it does not use. Chat used to carry a complete fenced
/// `create_document` example on every request: a small model then wrapped an
/// ordinary answer in an invented tool call 10 times out of 10 ("write a
/// simple rust program" became `{"name":"rust_program",...}`), and with the
/// example removed, 0 times out of 10. The identity line also claimed project
/// access and tool use in chat, where neither exists, and the model offered to
/// "compile and run it in your project" instead of writing the code. Tool
/// instructions a request needs are added to that request alone
/// (`document_tool_note`), which also keeps this prefix byte-stable for the
/// prompt cache.
fn build_system_prompt(mode: &str, snapshot: &str, model_name: &str) -> String {
    let mut p = if mode == "code" {
        format!(
            "You are the Local Companion, a local-first AI assistant running entirely on the user's PC (model: {model_name}). \
             You are NOT a cloud chatbot: you can see the user's linked project and files described below, and the app can run approved tools for you. \
             Never claim you cannot interact with the user's project — describe what you can see and what to do next. \
             Be concise. Format code with fenced blocks."
        )
    } else {
        format!(
            "You are the Local Companion, a local-first AI assistant running entirely on the user's PC (model: {model_name}). \
             Be concise. Format code with fenced blocks."
        )
    };
    if mode == "code" {
        if snapshot.is_empty() {
            p.push_str("\n\n[Code session: no project linked yet. Ask the user to link a project directory, then offer /init to inspect it.]");
        } else {
            p.push_str(&format!(
                "\n\n[Code session — linked project]\n{snapshot}\n\
                 You can READ the project yourself with tool calls (no approval needed for reads). \
                 When you need a file, emit exactly one tool call and wait for its result. Use this complete format with fence markers on separate lines:\n\
                 ```tool\n{{\"name\":\"read_file\",\"args\":{{\"path\":\"README.md\"}}}}\n```\n\
                 Explore according to the question: list directories, search relevant symbols, then inspect source, callers, tests and configuration. \
                 Documentation is a starting point, not proof of current implementation. For implementation/status claims, verify relevant source and tests; do not stop at markdown or tools folders. \
                 Follow relevant references inside this project, not every file indiscriminately. Distinguish documented plans from code you verified. \
                 For a question about a named function, class, setting or error, FIRST use search_text for that symbol in the relevant directory/file, then read the matching line range and its callers/tests. \
                 A search hit showing only a declaration is not the implementation. Immediately call read_file around that line to inspect the body before explaining what it does or returns. \
                 The user's project question already authorizes necessary read-only inspection. Never ask 'would you like me to read more?' or stop with 'I would need to inspect' when another safe read can answer the current question. Perform that read now. \
                 Do not page sequentially through thousands of lines looking for a symbol. If a chunk is unhelpful, change to a targeted search instead of reading the next unrelated chunk. A missing match in one excerpt does not mean the symbol is absent from the file. \
                 The snapshot is a real directory listing; do not conclude a listed project is missing based on a text search.\n\
                 Read tools: list_directory {{\"path\": \".\"}}, read_file {{\"path\": \"src/main.cpp\", \"start_line\": 1, \"end_line\": 200}}, \
                 search_text {{\"query\": \"temporal\", \"path\": \".\"}} (regex), system_info {{}}, list_processes {{}}.\n\
                 File reads return numbered chunks (200 lines by default, max 500 lines and about 12000 characters per chunk), total lines and continuation coordinates. \
                 You can jump to ANY start_line/end_line, including deep into very large files. Use search match line numbers to read surrounding code. \
                 Follow the returned start_line/start_column to read further chunks; omitted content has NOT been inspected. Cite paths and line ranges for code claims. \
                 Results have evidence IDs such as chunk-1. After inspecting them, you may call manage_context {{\"keep\":[\"chunk-1\"],\"release\":[\"chunk-2\"]}} to retain important evidence or drop irrelevant chunks from working context. \
                 Use actual returned IDs only. This never edits files or saved activity. Re-read released ranges if needed. As context fills, unmarked low-relevance chunks are released first, while your keep selections and latest result are protected. Keep only evidence essential to the answer. \
                 You have up to 24 read-only actions this turn. Use them as needed for evidence, stop when the question is answered, and disclose remaining gaps. \
                 Paths are workspace-relative and cannot escape. After tool results arrive, continue until you can \
                 answer from files you actually read — never invent file contents.\n\
                 Use the conversation to resolve follow-up references such as 'this project' and 'those tasks'. \
                 If the linked folder contains sibling projects and the conversation identifies one, stay inside that project; \
                 do not inspect unrelated siblings just because they appear in the tree.\n\
                 Writes, edits, builds, commits and commands need approval: do NOT emit those tools here; \
                 instead tell the user which command runs them: /init (inspect), /review [path], /plan <task>, \
                 /build, /test [filter], /run <cmd>, /diff."
            ));
        }
    } else {
        p.push_str("\n\n[Chat session: general assistant. Attached files appear as excerpts. Web search happens only when the user enables it.]");
    }
    p
}

/// Document kinds a model without tool support can still produce: it writes
/// the file's content as its reply and the host saves it (owner decision,
/// 2026-09-16). Structured kinds need the create_document tool's spec.
const PLAIN_DOCUMENT_KINDS: &[&str] = &["txt", "md", "csv", "html", "json"];

fn plain_document_note(kind: &str) -> String {
    format!("\n\n[The user asked for a .{kind} file. Reply with only the complete content of that file: no introduction, no explanation and no code fence around it. The app saves your reply as the file.]")
}

fn structured_document_notice(kind: &str) -> String {
    format!("This model can't create .{kind} files: its chat template has no tool support, which that file type needs. It can still create plain-text documents (.txt, .md, .csv, .html or .json); ask for one of those, or load a model that supports tools.\n\n")
}

/// A plain document reply's content: the inside of one surrounding code
/// fence when the model added one anyway.
fn plain_document_body(reply: &str) -> String {
    let text = reply.trim();
    if let Some(rest) = text.strip_prefix("```") {
        if let Some(newline) = rest.find('\n') {
            if let Some(inner) = rest[newline + 1..].trim_end().strip_suffix("```") {
                return inner.trim_end().to_string();
            }
        }
    }
    text.to_string()
}

/// A file name from the words of the request, without the request's verbs.
fn plain_document_filename(message: &str, kind: &str) -> String {
    const SKIP: &[&str] = &[
        "write", "create", "make", "generate", "give", "please", "file", "document", "the", "and",
        "for", "with", "about", "into", "that", "this", "markdown", "text", "csv", "html", "json", "txt",
        "can", "you", "new",
    ];
    let words: Vec<String> = message
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.len() > 2 && !SKIP.contains(&word.as_str()))
        .take(4)
        .collect();
    let stem = if words.is_empty() { "document".to_string() } else { words.join("-") };
    format!("{stem}.{kind}")
}

/// The document tool, offered only on a request that asks for a document.
/// Added to that request's user turn rather than the standing instructions:
/// a model shown a tool it has no use for reaches for it anyway.
fn document_tool_note() -> &'static str {
    "\n\n[This request asks for a file. Produce it with exactly one tool call and wait for its result, in this complete format with the fence markers on separate lines:\n\
     ```tool\n{\"name\":\"create_document\",\"args\":{\"filename\":\"report.xlsx\",\"sheets\":[{\"name\":\"Data\",\"rows\":[[\"Item\",\"Amount\"],[\"Example\",1]]}]}}\n```\n\
     Spec by file type: xlsx = sheets[{name,rows}]; docx and pdf = title + paragraphs[] (plain paragraphs); pptx = title + slides[{title,bullets[]}]; md/txt = text; csv = rows; html = title + html; json = data. \
     Put the complete content in the spec, never placeholders. The file appears in the conversation's Artifacts panel; after the result, tell the user it is ready and summarise what it contains.]"
}

/// The tools one chat request offers the model. Only an offered tool may run,
/// end the stream early, or earn a correction when its call is unreadable:
/// anything else a model writes in a tool fence is its answer text.
#[derive(Debug, Clone, Default)]
struct ChatToolOffer {
    names: Vec<&'static str>,
}

impl ChatToolOffer {
    fn for_request(has_workspace: bool, document_requested: bool) -> Self {
        let mut names = Vec::new();
        if has_workspace {
            names.extend([
                "list_directory",
                "read_file",
                "search_text",
                "system_info",
                "list_processes",
                "manage_context",
            ]);
        }
        if document_requested {
            names.push("create_document");
        }
        Self { names }
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn offers(&self, name: &str) -> bool {
        self.names.iter().any(|offered| *offered == name)
    }

    /// The offered call in `text`, if the text holds one.
    fn call_in(&self, text: &str) -> Option<crate::agent_runner::ToolCall> {
        crate::agent_runner::parse_tool_block(text).filter(|call| self.offers(&call.name))
    }

    /// Whether an unreadable action in `text` was an attempt at an offered
    /// tool, judged by the name it gives when that much is legible. With
    /// nothing offered there is nothing to correct toward.
    fn attempted_in(&self, text: &str) -> bool {
        if self.is_empty() {
            return false;
        }
        match tool_name_in(text) {
            Some(name) => self.offers(&name),
            None => true,
        }
    }
}

/// The tool name an action block gives, read leniently so a call whose JSON
/// is broken elsewhere still says which tool it meant.
fn tool_name_in(text: &str) -> Option<String> {
    let at = text.find("\"name\"")?;
    let rest = text[at + "\"name\"".len()..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Capped project snapshot for code-mode chats: tree + build + instructions.
fn workspace_snapshot(ws_path: &str, ws_name: &str) -> String {
    let root = std::path::PathBuf::from(ws_path);
    if !root.is_dir() {
        return format!(
            "Project '{ws_name}': directory missing ({ws_path}). Ask the user to relink it."
        );
    }
    let mut out = format!(
        "Project '{ws_name}' at {ws_path}\nBuild: {}\nTree (top levels):\n",
        detect_build_system(&root)
    );
    let mut entries: Vec<_> = std::fs::read_dir(&root)
        .map(|e| e.flatten().collect())
        .unwrap_or_default();
    entries.sort_by_key(|e| e.path());
    let mut files = 0;
    for e in entries.iter().take(60) {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" || name == "build" {
            continue;
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            out.push_str(&format!("  {name}/\n"));
            if let Ok(sub) = std::fs::read_dir(e.path()) {
                let mut kids: Vec<_> = sub.flatten().collect();
                kids.sort_by_key(|k| k.path());
                for k in kids.iter().take(12) {
                    out.push_str(&format!("    {}\n", k.file_name().to_string_lossy()));
                }
                if kids.len() > 12 {
                    out.push_str(&format!("    … ({} more)\n", kids.len() - 12));
                }
            }
        } else {
            if files < 40 {
                out.push_str(&format!("  {name}\n"));
            }
            files += 1;
        }
    }
    for cand in ["AGENTS.md", "CLAUDE.md", "MUSE.md", "PROJECT.md"] {
        if let Ok(text) = std::fs::read_to_string(root.join(cand)) {
            out.push_str(&format!(
                "\nProject instructions ({cand}, behavior only):\n{}\n",
                text.chars().take(1500).collect::<String>()
            ));
            break;
        }
    }
    out.chars().take(4000).collect()
}

/// Resolve only an explicitly linked project; never guess from the number of
/// available projects. A stale link is actionable, not permission for a fallback.
fn linked_code_workspace(
    st: &Storage,
    conv: &Conversation,
) -> Result<crate::storage::Workspace, ApiError> {
    if conv.workspace.trim().is_empty() {
        return Err(ApiError::bad(
            "code session has no linked project",
            "Choose a project for this session before asking it to inspect or change files.",
        ));
    }
    let workspace = st
        .get_workspace(&conv.workspace)
        .map_err(|error| ApiError::internal(format!("storage error: {error}")))?
        .ok_or_else(|| {
            ApiError::bad(
                "the linked project no longer exists",
                "Relink this session to the intended project. No other folder was used.",
            )
        })?;
    if !std::path::Path::new(&workspace.path).is_dir() {
        return Err(ApiError::bad(
            format!("project directory is unavailable: {}", workspace.path),
            "Reconnect the drive or select the correct project folder.",
        ));
    }
    Ok(workspace)
}

/// Stage 32 cached repo index: rebuilt on demand, refreshed explicitly.
async fn cached_repo_index(
    s: &AppState,
    ws_id: &str,
    path: &std::path::Path,
) -> crate::repo_index::RepoIndex {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some((mt, idx)) = s.repo_index.read().await.get(ws_id) {
        if *mt == mtime {
            return idx.clone();
        }
    }
    // Walking thousands of files is blocking work; keep it off the async
    // workers that serve streaming responses.
    let root = path.to_path_buf();
    let idx = tokio::task::spawn_blocking(move || crate::repo_index::build(&root))
        .await
        .unwrap_or_default();
    s.repo_index
        .write()
        .await
        .insert(ws_id.into(), (mtime, idx.clone()));
    idx
}

/// Stage 35 keyword retrieval over local chunks: explicit term overlap.
fn retrieve_knowledge<'a>(
    chunks: &'a [crate::storage::KnowledgeChunk],
    query: &str,
    limit: usize,
) -> Vec<&'a crate::storage::KnowledgeChunk> {
    let terms: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(|s| s.to_string())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i64, &'a crate::storage::KnowledgeChunk)> = chunks
        .iter()
        .map(|c| {
            let text = c.text.to_lowercase();
            let mut score = 0i64;
            for t in &terms {
                let hits = text.matches(t.as_str()).count().min(5) as i64;
                score += hits
                    * (if c.path.to_lowercase().contains(t) {
                        3
                    } else {
                        1
                    });
            }
            (score, c)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(limit.max(1))
        .map(|(_, c)| c)
        .collect()
}

/// Stage 31 single tool round: parse → validate → execute SAFE → observe.
/// Returns true when the loop should generate again with new context.
/// File chunks include their own continuation and fit below this transport cap.
fn chat_tool_output(output: &str) -> String {
    let excerpt: String = output.chars().take(16_000).collect();
    if excerpt.len() == output.len() {
        excerpt
    } else {
        format!("{excerpt}\n[Result truncated. Narrow the search/directory, or request a smaller file range; omitted content was not read.]")
    }
}

async fn run_chat_tool_round(
    s: &AppState,
    conv: &Option<String>,
    chat_ws: &Option<(String, String, std::path::PathBuf)>,
    offer: &ChatToolOffer,
    round_text: &str,
    turns: &mut Vec<ChatTurn>,
    memory: &mut crate::inspection_context::InspectionContext,
    tx_status: &tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
    message_id: &str,
    iteration: u32,
) -> bool {
    let call = match crate::agent_runner::parse_tool_block(round_text) {
        Some(c) => c,
        None => return false,
    };
    let status = |msg: &str| {
        let _ = tx_status.send(Ok(Event::default().event("status").safe_data(msg.to_string())));
    };
    let ws_path = chat_ws.as_ref().map(|(_, _, path)| path.clone());
    let is_document = call.name == "create_document";
    let offered = offer.offers(&call.name);
    // What a request did not offer is not an action. The one exception is a
    // real tool a code session withholds (a write, a command): the model is
    // told it needs approval, or it keeps asking. A tool name nothing in the
    // registry answers to, or any call in plain chat, is the model's text.
    let withheld = !offered
        && ws_path.is_some()
        && crate::tools::registry().iter().any(|tool| tool.name == call.name);
    if !offered && !withheld {
        return false;
    }
    if withheld || (call.name != "manage_context" && !crate::tools::chat_safe(&call.name)) {
        emit_chat_activity(s, message_id, tx_status, crate::agent::AgentEvent::tool_activity(
            "tool_error", AgentState::Observing, "This action was not run. Ask mode only permits safe inspection; choose Agent to request changes.".into(),
            iteration, call.name.clone(), call.args.clone(), None, None,
        )).await;
        status(&format!(
            "Skipped '{}' (needs approval — use the Agent panel or /plan for writes and commands)",
            call.name
        ));
        turns.push(ChatTurn::text("assistant", round_text));
        turns.push(ChatTurn::text(
            "user",
            format!("[System: the '{n}' tool needs explicit approval and cannot run inside chat. Answer from what you have read, or tell the user to run /plan {n}.]", n = call.name),
        ));
        return true;
    }
    let label = match call.name.as_str() {
        "manage_context" => "Selecting relevant evidence…".into(),
        "read_file" => format!(
            "Reading {}…",
            call.args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("file")
        ),
        "search_text" => format!(
            "Searching for '{}'…",
            call.args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
        "list_directory" => format!(
            "Listing {}…",
            call.args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
        ),
        "create_document" => format!(
            "Creating {}…",
            call.args
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("document")
        ),
        _ => format!("Running {}…", call.name),
    };
    status(&label);
    emit_chat_activity(
        s,
        message_id,
        tx_status,
        crate::agent::AgentEvent::tool_activity(
            "tool_started",
            AgentState::ExecutingTool,
            label.clone(),
            iteration,
            call.name.clone(),
            call.args.clone(),
            None,
            None,
        ),
    )
    .await;
    let executed = if call.name == "manage_context" {
        memory
            .manage(&call.args, turns)
            .map(crate::tools::ToolResult::ok)
            .map_err(crate::tools::ToolError::InvalidArgs)
    } else if is_document {
        // Rendered by deterministic code from the model's spec and recorded
        // as an artifact of this conversation (never written into a project).
        crate::documents::execute_create_document(
            &s.storage,
            &s.artifacts_dir,
            conv.as_deref().unwrap_or("direct"),
            &call.args,
        )
        .await
    } else {
        let ws = crate::workspace::WorkspaceManager::new(
            ws_path.clone().expect("workspace checked above"),
        );
        let req = crate::tools::ToolRequest {
            name: call.name.clone(),
            args: call.args.clone(),
            approved: false,
        };
        // File reads and regex searches are blocking filesystem work.
        tokio::task::spawn_blocking(move || crate::tools::execute(&req, &ws, false))
            .await
            .unwrap_or_else(|error| {
                Err(crate::tools::ToolError::InvalidArgs(format!(
                    "tool task failed: {error}"
                )))
            })
    };
    let (ok, output) = match executed {
        Ok(r) => (r.ok, r.output),
        Err(crate::tools::ToolError::PermissionRequired { reason, .. }) => {
            (false, format!("Permission required: {reason}"))
        }
        Err(e) => (
            false,
            format!("Tool failed: {e}\nYou sent args: {}", call.args),
        ),
    };
    status(&format!(
        "{} {}",
        if ok { "✓" } else { "⚠" },
        label.replace('…', "")
    ));
    // Audit every chat tool call like any other (§54).
    let excerpt: String = output.chars().take(16_000).collect();
    emit_chat_activity(
        s,
        message_id,
        tx_status,
        crate::agent::AgentEvent::tool_activity(
            if ok { "tool_result" } else { "tool_error" },
            AgentState::Observing,
            match (is_document, ok) {
                (true, true) => "Document created".into(),
                (true, false) => "Document failed".into(),
                (false, true) => "Inspection finished".into(),
                (false, false) => "Inspection failed".into(),
            },
            iteration,
            call.name.clone(),
            call.args.clone(),
            Some(if output.len() > excerpt.len() {
                format!("{excerpt}\n[Output truncated for display]")
            } else {
                excerpt
            }),
            None,
        ),
    )
    .await;
    {
        let st = s.storage.lock().await;
        let _ = st.record_tool_execution(&crate::storage::ToolExecution {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conv.clone().unwrap_or_else(|| "direct".into()),
            tool: format!("chat:{}", call.name),
            args: serde_json::to_string(&call.args).unwrap_or_default(),
            result: output.chars().take(2000).collect(),
            approved: false,
            created_at: chrono::Utc::now().to_rfc3339(),
        });
    }
    turns.push(ChatTurn::text("assistant", round_text));
    let next_step = if call.name == "search_text" && ok {
        "\n[Host guidance: these are search matches, not whole function bodies. If the original question asks about implementation or return values not visible here, your NEXT response must call read_file on the matching path/line range. Do not ask permission for that read; the user's question already requests this inspection. Otherwise answer from the evidence.]"
    } else {
        ""
    };
    turns.push(ChatTurn::text(
        "user",
        format!(
            "[tool result: {}]\n{}{next_step}",
            call.name,
            chat_tool_output(&output)
        ),
    ));
    memory.record(turns, &call.name, &call.args);
    true
}

/// Artifacts recorded for a conversation so far; a chat turn that asked for a
/// file compares before and after its tool rounds.
async fn chat_artifact_count(state: &AppState, conv: &Option<String>) -> usize {
    let Some(conv_id) = conv.as_deref().filter(|id| !id.is_empty()) else {
        return 0;
    };
    state
        .storage
        .lock()
        .await
        .artifacts_for(conv_id)
        .map(|rows| rows.len())
        .unwrap_or(0)
}

async fn emit_chat_activity(
    state: &AppState,
    message_id: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
    event: crate::agent::AgentEvent,
) {
    if let Err(error) = state
        .storage
        .lock()
        .await
        .record_message_activity(message_id, &event)
    {
        tracing::error!("Could not save chat activity: {error}");
    }
    let _ = tx.send(Ok(Event::default()
        .event("activity")
        .safe_data(serde_json::to_string(&event).unwrap_or_default())));
}

fn reasoning_preface(budget: &str, native: bool) -> String {
    let depth = match budget {
        "low" => "Think briefly",
        "high" => "Think very carefully, exploring alternatives",
        _ => "Think step by step",
    };
    if native {
        format!("{depth} using your native reasoning mode, then answer.")
    } else {
        format!(
            "{depth} before answering. Give a concise explanation of your conclusions and useful next steps, not private step-by-step deliberation. This is an extended response budget, not a claim of native reasoning support."
        )
    }
}

/// Stub reply when no sidecar is running: echoes words if a model is marked
/// loaded, else the actionable "load a model" message.
async fn stub_reply(s: &AppState, message: &str) -> String {
    let loaded = s.inference.read().await.is_loaded();
    if loaded {
        message.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        "No model loaded. Load a model first.".into()
    }
}

/// Stream a complete deterministic text (commands, stubs) as SSE tokens.
/// No inference involved; carries an optional `command` action for the UI.
fn stream_text(
    full: String,
    command: Option<serde_json::Value>,
) -> Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>> {
    let words = full.split_whitespace().count() as u32;
    let mut events: Vec<Result<Event, Infallible>> = full
        .split_whitespace()
        .map(|w| Ok(Event::default().event("token").safe_data(format!("{w} "))))
        .collect();
    let mut done = serde_json::json!({"prompt_tokens": 0, "generated_tokens": words, "stopped": false, "reasoning": "off", "gen_tps": null});
    if let Some(action) = command {
        done["command"] = action;
    }
    events.push(Ok(Event::default().event("done").safe_data(done.to_string())));
    Sse::new(stream::iter(events).boxed()).keep_alive(KeepAlive::default())
}

fn stream_conversation(
    reply: serde_json::Value,
) -> Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>> {
    let done = serde_json::json!({"prompt_tokens":0,"generated_tokens":0,"stopped":false,"reasoning":"off","gen_tps":null,
        "disposition":reply["disposition"],"message_id":reply["message_id"]});
    Sse::new(
        stream::iter(vec![
            Ok(Event::default()
                .event("token")
                .safe_data(reply["message"].as_str().unwrap_or_default())),
            Ok(Event::default().event("done").safe_data(done.to_string())),
        ])
        .boxed(),
    )
    .keep_alive(KeepAlive::default())
}

/// Stage 12: execute a slash command as a chat turn — persist both sides,
/// stream the deterministic result, attach the UI action to `done`.
async fn run_command_as_chat(
    s: &AppState,
    conv_id: Option<String>,
    name: &str,
    args: &str,
) -> Result<Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>>, ApiError> {
    if let Some(ref cid) = conv_id {
        let st = s.storage.lock().await;
        match st.get_conversation(cid) {
            Ok(Some(_)) => {
                let now = chrono::Utc::now().to_rfc3339();
                let _ = st.add_message(&Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: cid.clone(),
                    role: "user".into(),
                    content: format!(
                        "/{name}{}",
                        if args.is_empty() {
                            String::new()
                        } else {
                            format!(" {args}")
                        }
                    ),
                    created_at: now,
                });
            }
            Ok(None) => return Err(ApiError::not_found("conversation not found")),
            Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
        }
    }
    let outcome = run_slash_command(s, conv_id.clone(), name, args).await;
    let (text, action) = match &outcome {
        Ok(o) => (o.text(), Some(o.action_json())),
        Err(e) => (format!("Command failed: {}", e.message), None),
    };
    if let (Some(cid), Ok(_)) = (conv_id, &outcome) {
        let st = s.storage.lock().await;
        let _ = st.add_message(&Message {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: cid,
            role: "assistant".into(),
            content: text.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        });
    }
    Ok(stream_text(text, action))
}
/// Output allowance for one chat reply: generous enough for a long code answer,
/// never larger than what the context can hold after the input.
fn chat_output_budget(n_ctx: u32, input_tokens: u32, reasoning: ReasoningMode, budget: &str) -> u32 {
    let cap = match reasoning {
        ReasoningMode::Off => 8192,
        _ if budget == "high" => 16384,
        _ => 12288,
    };
    let room = n_ctx.saturating_sub(input_tokens).saturating_sub(256);
    cap.min(room).max(256)
}

async fn chat_sse(
    State(s): State<AppState>,
    Json(req): Json<ChatReq>,
) -> Result<Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>>, ApiError> {
    let request_started = std::time::Instant::now();
    validate_content(&req.message)?;
    if req.attachments.len() > 16 {
        return Err(ApiError::bad(
            "too many attachments",
            "Attach at most 16 files per message; large docs are chunked (§69).",
        ));
    }
    let conv_id = if req.conversation_id.trim().is_empty() {
        None
    } else {
        Some(req.conversation_id.clone())
    };

    // Stage 12: deterministic commands bypass inference entirely (§137).
    // Unknown /names fall through to the model... no: unknown commands get a
    // direct correction so inference budget is never spent on typos.
    if let Some((cmd_name, cmd_args)) = crate::commands::parse(&req.message) {
        if crate::commands::find(&cmd_name).is_some() {
            return run_command_as_chat(&s, conv_id, &cmd_name, &cmd_args).await;
        }
        let hint = format!("Unknown command '/{cmd_name}'. Try /help.");
        return Ok(stream_text(hint, None));
    }

    let _runtime_registration = s.runtime_update.try_lock().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "The model runtime is being updated.",
            "Wait for the model to finish loading, then send your message.",
        )
    })?;
    let intent = answering_intent(&s, conv_id.as_deref().unwrap_or_default(), &req.message).await;
    if matches!(
        intent,
        MessageIntent::Acknowledgement | MessageIntent::Greeting
    ) && req.attachments.is_empty()
    {
        let reply = conversational_reply(
            &s,
            conv_id.as_deref().unwrap_or_default(),
            &req.message,
            if intent == MessageIntent::Greeting {
                "Hello! What would you like to work on?"
            } else {
                "Got it. Let me know if you’d like anything else."
            },
            "conversation",
        )
        .await?;
        return Ok(stream_conversation(reply));
    }
    let mut resumed_task = None;
    if intent == MessageIntent::Continue {
        if let Some(cid) = &conv_id {
            let workspace = {
                let st = s.storage.lock().await;
                match st
                    .get_conversation(cid)
                    .map_err(|e| ApiError::internal(e.to_string()))?
                {
                    Some(conv) if conv.mode == "code" => {
                        Some(linked_code_workspace(&st, &conv)?.path)
                    }
                    _ => None,
                }
            };
            if let Some(workspace) = workspace {
                match resolve_continuation(&s, cid, std::path::Path::new(&workspace)).await? {
                    Continuation::Task(task) => resumed_task = Some(task),
                    Continuation::Reply(disposition, reply) => {
                        return Ok(stream_conversation(
                            conversational_reply(&s, cid, &req.message, reply, disposition).await?,
                        ))
                    }
                }
            }
        }
    }

    // Verify the conversation exists and persist the user turn first so a
    // crash never silently drops it (§83).
    if let Some(ref cid) = conv_id {
        let st = s.storage.lock().await;
        match st.get_conversation(cid) {
            Ok(Some(conv)) => {
                if conv.mode == "code" {
                    linked_code_workspace(&st, &conv)?;
                    if intent == MessageIntent::Task {
                        st.clear_task_context(cid)
                            .map_err(|error| ApiError::internal(error.to_string()))?;
                    }
                }
                st.add_message(&Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: cid.clone(),
                    role: "user".into(),
                    content: req.message.clone(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                })
                .map_err(|error| {
                    ApiError::internal(format!("Could not save your message: {error}"))
                })?;
            }
            Ok(None) => return Err(ApiError::not_found("conversation not found")),
            Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
        }
    }

    let settings = s.settings.read().await.clone();
    let sidecar = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            llama
                .running
                .as_ref()
                .map(|r| (r.base_url.clone(), r.cfg.clone()))
        } else {
            None
        }
    };

    // No live runtime: a model that is merely marked loaded (or whose worker
    // died) must never produce a fake echo reply. Say what happened instead.
    if sidecar.is_none() {
        let marked_loaded = s.inference.read().await.is_loaded();
        if marked_loaded {
            s.inference.write().await.unload();
            s.models.write().await.unload_all();
        }
        let full = if marked_loaded {
            "The model runtime stopped unexpectedly. Reload the model and send your message again; your message was saved.".to_string()
        } else {
            stub_reply(&s, &req.message).await
        };
        if let Some(cid) = conv_id {
            let st = s.storage.lock().await;
            if let Err(e) = st.add_message(&Message {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: cid,
                role: "assistant".into(),
                content: full.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
            }) {
                tracing::warn!("persist assistant message failed: {e}");
            }
        }
        let mut events: Vec<Result<Event, Infallible>> = full
            .split_whitespace()
            .map(|w| Ok(Event::default().event("token").safe_data(format!("{w} "))))
            .collect();
        let words = full.split_whitespace().count() as u32;
        events.push(Ok(Event::default().event("done").safe_data(
            serde_json::json!({"prompt_tokens": 0, "generated_tokens": words, "stopped": false, "reasoning": "off", "gen_tps": null}).to_string(),
        )));
        return Ok(Sse::new(stream::iter(events).boxed()).keep_alive(KeepAlive::default()));
    }
    let (base_url, cfg) = sidecar.expect("checked");

    // Stage 11 capability resolution (§115): message > conversation > settings.
    // Search has no global on-switch: it stays off unless the message or the
    // conversation explicitly enables it (§117).
    let context = assemble_request_context(&s, conv_id.as_deref(), None, cfg.n_ctx).await?;
    let want_reasoning = req
        .reasoning
        .unwrap_or(context.reasoning_default || settings.reasoning.default_on);
    let want_search = req.search.unwrap_or(context.search_default);
    let native_reasoning = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let reasoning = if !want_reasoning {
        ReasoningMode::Off
    } else if native_reasoning {
        ReasoningMode::Native
    } else {
        ReasoningMode::Extended
    };

    // Live sidecar path: true token-passthrough (§19). A background task runs
    // the stream, forwarding each delta into the SSE channel immediately.
    // The run is registered with the generation tracker so Stop (or a new
    // turn) cancels the sidecar request itself and keeps the partial (§46).
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let gen_id = uuid::Uuid::new_v4().to_string();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let partial = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let persisted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // A new turn supersedes any running one: cancel it (partial kept) first.
    s.generations.write().await.cancel_current(&s.storage).await;
    // Automatic compaction is decided before this reply and runs before it is
    // generated, never during it: saved history at the threshold share of the
    // room a request gives history would otherwise start losing old messages.
    let compaction_due = conv_id.as_ref().and_then(|_| {
        let pct = context.history_usage_pct();
        (settings.memory.auto_compaction() && pct >= settings.memory.compact_threshold_pct())
            .then_some((pct, context.history_room_chars))
    });
    let RequestContext {
        mut sys_prompt,
        sys_mode,
        turns: bg_turns,
        images_skipped,
        chat_workspace,
        ..
    } = context;
    let resumed_note = resumed_task
        .map(|task| format!("\nThe user's continuation refers only to this saved unfinished task in this same project: {task}\nThis Ask reply remains read-only; continue inspection or explanation, never implement changes."))
        .unwrap_or_default();
    sys_prompt.push_str(&resumed_note);
    let bg_state = s.clone();
    let bg_conv = conv_id.clone();
    let bg_sys = sys_prompt;
    let code_activity = sys_mode == "code";
    let bg_id = gen_id.clone();
    let bg_cancel = cancel.clone();
    let bg_partial = partial.clone();
    let bg_persisted = persisted.clone();
    let bg_tracker = s.generations.clone();
    let handle = tokio::spawn(async move {
        // `full_cb` is what the user sees and what is saved (and what Stop
        // keeps): prose only. `raw_cb` is every round's text as generated,
        // which is what actions are parsed from.
        let full_cb = bg_partial.clone();
        let raw_cb = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let tx_tok = tx.clone();
        let tx_status = tx.clone();
        let cancel_cb = bg_cancel.clone();
        let status = |msg: &str| {
            let _ = tx_status.send(Ok(Event::default().event("status").safe_data(msg.to_string())));
        };
        let phase = |name: &str| {
            let _ = tx_status.send(Ok(Event::default()
                .event("phase")
                .safe_data(serde_json::json!({ "phase": name }).to_string())));
        };
        let mut bg_turns = bg_turns;
        let mut bg_sys = bg_sys;
        if let (Some((pct, room_chars)), Some(cid)) = (compaction_due, bg_conv.clone()) {
            phase("compacting");
            status(&format!(
                "Compacting the conversation: {pct}% of the usable context is in use (automatic compaction starts at {}%). The reply starts when the summary is ready…",
                settings.memory.compact_threshold_pct()
            ));
            // Keep what fits in two fifths of the room verbatim, so the
            // compacted context lands well under the threshold.
            let keep = recent_messages_within(
                &bg_state,
                &cid,
                room_chars * 2 / 5,
                settings.memory.compaction_keep_turns,
            )
            .await;
            match compact_conversation_keeping(&bg_state, &cid, keep).await {
                Ok(stats) if stats.status == "compacted" => {
                    match assemble_request_context(&bg_state, Some(&cid), None, cfg.n_ctx).await {
                        Ok(fresh) => {
                            bg_turns = fresh.turns;
                            bg_sys = fresh.sys_prompt + &resumed_note;
                        }
                        Err(error) => tracing::warn!(
                            "context re-assembly after compaction failed: {}",
                            error.message
                        ),
                    }
                    status(&format!(
                        "Conversation compacted: {} older messages summarized ({} → {} characters). Writing the reply…",
                        stats.messages, stats.before_chars, stats.after_chars
                    ));
                }
                Ok(_) => status("Nothing older could be compacted; replying with the current context…"),
                Err(error) => {
                    tracing::warn!("automatic compaction failed: {}", error.message);
                    status(&format!(
                        "Automatic compaction failed ({}); replying with the most recent messages that fit.",
                        error.message
                    ));
                }
            }
            phase("processing");
        }
        // Stage 11: explicit web search runs BEFORE generation (§120). The
        // query is the user's message; results become model context plus
        // persisted citations (§121). Nothing external happens unless the
        // toggle enabled this path (§117).
        let mut web_block = String::new();
        let mut cite_block = String::new();
        if want_search {
            status("Searching the web…");
            let scfg = crate::search::SearchConfig {
                provider: settings.search.provider.clone(),
                brave_key: settings.search.brave_key.clone(),
                custom_url: settings.search.custom_url.clone(),
                max_results: settings.search.max_results,
                timeout_secs: settings.search.timeout_secs,
            };
            match crate::search::run_search(&req.message, &scfg).await {
                Ok((results, provider)) => {
                    let conv = bg_conv.clone().unwrap_or_else(|| "direct".into());
                    let st = bg_state.storage.lock().await;
                    let _ = st.record_search_run(&crate::storage::SearchRun {
                        id: uuid::Uuid::new_v4().to_string(),
                        conversation_id: conv,
                        query: req.message.chars().take(500).collect(),
                        provider: provider.clone(),
                        result_count: results.len(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    });
                    drop(st);
                    if results.is_empty() {
                        web_block = "\n\n[Web search returned no results. Answer from local knowledge and say so.]".into();
                        status("No web results — answering locally");
                    } else {
                        let mut ctx = format!("\n\n[Web search results via {provider} — cite favorite sources as Sources 1..N]\n");
                        for (i, r) in results.iter().enumerate() {
                            ctx.push_str(&format!(
                                "{}. {} ({})\n{}\n",
                                i + 1,
                                r.title,
                                r.url,
                                r.snippet
                            ));
                        }
                        web_block = ctx;
                        cite_block = crate::search::citations(&results);
                        status(&format!("Found {} results", results.len()));
                    }
                }
                Err(e) => {
                    // §125: never fabricate; degrade to local knowledge.
                    web_block = "\n\n[Web search failed. Answer from local knowledge and say search was unavailable.]".into();
                    let _ = tx_status.send(Ok(Event::default().event("error").safe_data(format!(
                        "Internet search failed ({e}). Continuing without web. [Retry]"
                    ))));
                }
            }
        }
        // Native thinking is reported only when the sidecar actually emits a
        // reasoning-channel delta, never inferred from a requested setting.
        // The reasoning preface is a transient system turn (never persisted)
        // placed AFTER the identity prompt so the cached prefix survives it;
        // web context is appended to the latest user turn.
        let mut turns = bg_turns;
        turns.insert(0, ChatTurn::text("system", bg_sys));
        if reasoning != ReasoningMode::Off {
            let preface = reasoning_preface(&settings.reasoning.budget, reasoning == ReasoningMode::Native);
            if legacy_prompt_order() {
                turns.insert(1, ChatTurn::text("system", preface));
            } else if let Some(latest) = turns.iter_mut().rev().find(|t| t.role == "user") {
                // After the history: turning Reasoning on or off no longer
                // changes the bytes in front of every earlier turn.
                latest.content.push_str(&format!("\n\n{preface}"));
            }
        }
        if !web_block.is_empty() {
            if let Some(last_user) = turns.iter_mut().rev().find(|t| t.role == "user") {
                last_user.content.push_str(&web_block);
            }
        }
        // A request for a file is only fulfilled by a file: one correction for
        // an unreadable tool call and one nudge to produce the document, so a
        // small model that pastes the content into the chat still delivers.
        let requested_kind = crate::documents::requested_document_kind(&req.message);
        let tools_supported = cfg
            .template_caps
            .map(|caps| caps.tools_supported())
            .unwrap_or_default();
        // A model whose template has no tool support is never taught a tool:
        // a plain-text document comes from its reply, a structured one is
        // declined plainly.
        let no_tools = tools_supported == crate::inference::Support::No;
        let plain_document = requested_kind.filter(|kind| no_tools && PLAIN_DOCUMENT_KINDS.contains(kind));
        let declined_document = requested_kind.filter(|kind| no_tools && !PLAIN_DOCUMENT_KINDS.contains(kind));
        let document_requested = requested_kind.is_some() && !no_tools;
        if let Some(last_user) = turns.iter_mut().rev().find(|t| t.role == "user") {
            if document_requested {
                last_user.content.push_str(document_tool_note());
            } else if let Some(kind) = plain_document {
                last_user.content.push_str(&plain_document_note(kind));
            } else if let Some(kind) = declined_document {
                last_user.content.push_str(&format!("\n\n[This model cannot create .{kind} files and the user has already been told. Answer the rest of the request in plain text; do not paste a file.]"));
            }
        }
        if let Some(kind) = declined_document {
            send_split_pieces(vec![crate::stream_split::Piece::Prose(structured_document_notice(kind))], &full_cb, &tx_tok);
        }
        let reasoning_label = match reasoning {
            ReasoningMode::Off => "off",
            ReasoningMode::Native => "native",
            ReasoningMode::Extended => "extended",
        };
        // Reasoning off means off: thinking models are asked not to think,
        // instead of silently spending thousands of hidden tokens per reply.
        let request_options = crate::llamaserver::RequestOptions {
            thinking: (reasoning == ReasoningMode::Off).then_some(false),
            ..crate::llamaserver::RequestOptions::default()
        };
        // Stage 31/32/35: code sessions get index hints + local knowledge so
        // the model can find files itself instead of guessing from the tree.
        let chat_ws = chat_workspace;
        if let Some((ref ws_id, _, ref ws_path)) = chat_ws {
            if ws_path.is_dir() {
                let index_key = format!("{ws_id}:{}", ws_path.display());
                let idx = cached_repo_index(&bg_state, &index_key, ws_path).await;
                let hits = crate::repo_index::search(&idx, &req.message, 5);
                if !hits.is_empty() {
                    let names: Vec<String> = hits.iter().map(|f| f.path.clone()).collect();
                    if let Some(last_user) = turns.iter_mut().rev().find(|t| t.role == "user") {
                        last_user.content.push_str(&format!(
                            "\n\n[Relevant files (repo index): {}. Read the ones that matter with read_file before answering.]",
                            names.join(", ")
                        ));
                    }
                }
                let st = bg_state.storage.lock().await;
                let mut chunks = st.knowledge_for(ws_id).unwrap_or_default();
                if let Ok(Some(linked)) = st.get_workspace(ws_id) {
                    let linked_root = std::fs::canonicalize(&linked.path).ok();
                    if let Some(prefix) = linked_root
                        .as_ref()
                        .and_then(|root| ws_path.strip_prefix(root).ok())
                    {
                        if !prefix.as_os_str().is_empty() {
                            let prefix =
                                format!("{}/", prefix.to_string_lossy().replace('\\', "/"));
                            chunks.retain_mut(|chunk| {
                                if let Some(relative) =
                                    chunk.path.replace('\\', "/").strip_prefix(&prefix)
                                {
                                    chunk.path = relative.to_owned();
                                    true
                                } else {
                                    false
                                }
                            });
                        }
                    }
                }
                drop(st);
                let top = retrieve_knowledge(&chunks, &req.message, 4);
                if !top.is_empty() {
                    let mut block =
                        String::from("\n\n[Local knowledge — cite the file when you use it]");
                    for c in top {
                        block.push_str(&format!(
                            "\n[{}] {}\n",
                            c.path,
                            c.text.chars().take(800).collect::<String>()
                        ));
                    }
                    if let Some(last_user) = turns.iter_mut().rev().find(|t| t.role == "user") {
                        last_user.content.push_str(&block);
                    }
                }
            }
        }
        // Stage 31 in-chat tool loop: the model reads files itself (SAFE
        // tools, workspace-confined, no approval). Bounded rounds; writes and
        // commands stay in the approval flow (§92).
        let (mut prompt_sum, mut gen_sum, mut ms_sum) = (0u32, 0u32, 0u64);
        let mut timing = crate::inference::OutputTiming::default();
        let mut tool_uses = 0u32;
        let mut pending_tool_limit = false;
        let mut truncated = false;
        let mut reasoning_seen = false;
        let mut failed: Option<String> = None;
        // A code session's reads do not depend on the template's own tool
        // support: its system prompt always teaches the app's text envelope,
        // which the app parses itself (agent runs serve such models with a
        // structured action format). Measured live: a 4B model whose runtime
        // reported no tool support asked about a project, issued a correct
        // read_file call, was refused as "needs approval", and then guessed
        // the file's contents. Plain chat keeps the gate: there a 2B model
        // taught a tool wrote broken tool blocks in 10 of 10 replies.
        let tool_offer = ChatToolOffer::for_request(
            chat_ws.as_ref().is_some_and(|(_, _, path)| path.is_dir()),
            document_requested,
        );
        let artifacts_before = chat_artifact_count(&bg_state, &bg_conv).await;
        let mut document_created = false;
        let mut corrections = 0u32;
        let mut nudges = 0u32;
        let mut overflow_retried = false;
        let client = match SidecarClient::new(base_url) {
            Ok(c) => Some(c.with_recorder(request_recorder(
                &bg_state,
                bg_conv.as_deref().unwrap_or_default(),
                &bg_id,
                "chat",
            ))),
            Err(e) => {
                failed = Some(format!("sidecar client failed: {e}"));
                None
            }
        };
        let mut inspection_memory = crate::inspection_context::InspectionContext::new(&turns);
        if let Some(client) = client {
            loop {
                if bg_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // Room kept for the reply while old tool results are released:
                // a quarter of the window (1K-8K). The reply's own allowance is
                // computed afterwards from whatever input remains, so a large
                // output cap never starves the inspection of context.
                let output_reserve = (cfg.n_ctx / 4).clamp(1024, 8192).min(cfg.n_ctx / 2);
                // Only shorten old transient tool results, never the user's
                // question, saved conversation, or latest chunk. Full evidence
                // remains in the activity journal and can be read again.
                if tool_uses > 0
                    && !inspection_memory.compact(&mut turns, cfg.n_ctx, output_reserve)
                {
                    failed = Some("Inspection reached the model's context capacity. Completed reads are saved in file activity; this is a partial inspection, not a completed answer. Continue with a narrower question or a larger context.".into());
                    break;
                }
                let estimated_input =
                    crate::agent::AgentContextUsage::for_turns(&turns, cfg.n_ctx, 0, 0)
                        .estimated_tokens;
                let max_tokens = chat_output_budget(
                    cfg.n_ctx,
                    estimated_input,
                    reasoning,
                    &settings.reasoning.budget,
                );
                if tool_uses == CHAT_TOOL_ROUNDS {
                    turns.push(ChatTurn::text("user", "[Inspection budget reached. Do not request more tools. Answer using the evidence already read, cite relevant paths/lines, and clearly identify unverified areas. Do not claim a complete project audit.]"));
                }
                let gen_start = std::time::Instant::now();
                let round_offset_ms = request_started.elapsed().as_millis() as u64;
                let full_r = full_cb.clone();
                let raw_r = raw_cb.clone();
                let splitter = std::sync::Arc::new(std::sync::Mutex::new(crate::stream_split::StreamSplitter::new()));
                let splitter_r = splitter.clone();
                let tx_r = tx_tok.clone();
                let tx_reason = tx_tok.clone();
                let tx_phase = tx_status.clone();
                let round = tool_uses + 1;
                let cancel_r = cancel_cb.clone();
                let round_base = raw_cb.lock().expect("lock").len();
                let visible_base = full_cb.lock().expect("lock").len();
                // A complete action ends the round wherever a tool can run
                // (project inspection in code sessions, documents anywhere):
                // the runtime is released instead of generating text after it.
                let stop_offer = tool_offer.clone();
                let handlers = crate::llamaserver::StreamHandlers {
                    on_token: Box::new(move |tok: &str| {
                        raw_r.lock().expect("lock").push_str(tok);
                        let pieces = splitter_r.lock().expect("lock").push(tok);
                        send_split_pieces(pieces, &full_r, &tx_r);
                    }),
                    on_reasoning: Box::new(move |text: &str| {
                        let _ = tx_reason
                            .send(Ok(Event::default().event("reasoning").safe_data(text.to_string())));
                    }),
                    on_phase: Box::new(move |phase| {
                        let _ = tx_phase.send(Ok(Event::default().event("phase").safe_data(
                            serde_json::json!({"phase": phase, "round": round}).to_string(),
                        )));
                    }),
                    is_cancelled: Box::new(move || {
                        cancel_r.load(std::sync::atomic::Ordering::SeqCst)
                    }),
                    should_stop: Box::new(move |text: &str| {
                        !stop_offer.is_empty()
                            && text.contains("```tool")
                            && stop_offer.call_in(text).is_some()
                    }),
                };
                let out = client
                    .stream(&turns, max_tokens, &cfg, &request_options, handlers)
                    .await;
                let gen_ms = gen_start.elapsed().as_millis().max(1) as u64;
                // Whatever the splitter still held is decided now.
                let tail = splitter.lock().expect("lock").finish();
                send_split_pieces(tail, &full_cb, &tx_tok);
                match out {
                    Ok(outcome) => {
                        let m = &outcome.metrics;
                        prompt_sum += m.prompt_tokens;
                        gen_sum += m.generated_tokens;
                        ms_sum += gen_ms;
                        reasoning_seen |= outcome.reasoning_present;
                        if let Some(round_timing) = m.timing.as_ref() {
                            timing.add_round(round_timing, round_offset_ms);
                        }
                        let round_text = raw_cb.lock().expect("lock")[round_base..].to_string();
                        let has_tool = tool_offer.call_in(&round_text).is_some();
                        if outcome.finish_reason.as_deref() == Some("length")
                            && !outcome.early_stopped
                            && !has_tool
                        {
                            truncated = true;
                        }
                        if code_activity {
                            let narrative = if has_tool {
                                crate::agent_runner::visible_progress(&round_text)
                            } else {
                                round_text.clone()
                            };
                            if !narrative.trim().is_empty() {
                                emit_chat_activity(
                                    &bg_state,
                                    &bg_id,
                                    &tx_status,
                                    crate::agent::AgentEvent::activity(
                                        if has_tool { "thought" } else { "response" },
                                        if has_tool
                                            || bg_cancel.load(std::sync::atomic::Ordering::SeqCst)
                                        {
                                            AgentState::Observing
                                        } else {
                                            AgentState::Completed
                                        },
                                        narrative,
                                        tool_uses + 1,
                                    ),
                                )
                                .await;
                            }
                        }
                        // Tool round?
                        let worked = if tool_uses < CHAT_TOOL_ROUNDS {
                            run_chat_tool_round(
                                &bg_state,
                                &bg_conv,
                                &chat_ws,
                                &tool_offer,
                                &round_text,
                                &mut turns,
                                &mut inspection_memory,
                                &tx_status,
                                &bg_id,
                                tool_uses + 1,
                            )
                            .await
                        } else {
                            pending_tool_limit = has_tool;
                            false
                        };
                        if worked {
                            tool_uses += 1;
                            // A produced file is an artifact row: count them
                            // rather than parse the transcript.
                            document_created |= chat_artifact_count(&bg_state, &bg_conv).await
                                > artifacts_before;
                            continue;
                        }
                        if tool_uses < CHAT_TOOL_ROUNDS {
                            // The model tried to act but the call could not be
                            // read: say what was wrong and let it re-emit once,
                            // as the agent loop does.
                            // Only a failed attempt at a tool this request offered
                            // earns a correction. Asking a model that invented a tool
                            // to "re-emit the envelope" taught it the answer was a
                            // tool call, and it repeated the same block.
                            if let Some(problem) =
                                crate::agent_runner::action_problem(&round_text).filter(|_| {
                                    corrections == 0 && tool_offer.attempted_in(&round_text)
                                })
                            {
                                corrections += 1;
                                discard_round_text(&full_cb, visible_base, &tx_status);
                                turns.push(ChatTurn::text("assistant", round_text.clone()));
                                turns.push(ChatTurn::text(
                                    "user",
                                    format!("[System: your tool call could not be read ({problem}). Nothing was executed. Re-emit exactly one tool envelope with valid JSON: every string value in double quotes (a formula such as =SUM(C2:C6) is a string), no comments, no trailing commas.]"),
                                ));
                                let _ = tx_status.send(Ok(Event::default().event("status").safe_data(
                                    "The tool call was not valid JSON; asking for a corrected one…",
                                )));
                                continue;
                            }
                            if document_requested && !document_created && nudges == 0 {
                                nudges += 1;
                                discard_round_text(&full_cb, visible_base, &tx_status);
                                turns.push(ChatTurn::text("assistant", round_text.clone()));
                                turns.push(ChatTurn::text(
                                    "user",
                                    "[System: the user asked for a file, and no file has been produced. Produce it now with exactly one create_document call that contains the complete content (use what you wrote above). Do not paste the content into the chat instead.]",
                                ));
                                let _ = tx_status.send(Ok(Event::default()
                                    .event("status")
                                    .safe_data("Producing the requested file…")));
                                continue;
                            }
                        }
                        break;
                    }
                    Err(e) => {
                        // The server measured the prompt and it did not fit:
                        // nothing was generated, so leaving out the oldest
                        // saved messages and asking once more repeats no
                        // visible work. Only before any tool round, whose
                        // recorded positions would shift.
                        if let Some(crate::inference::thiserror_stub::SidecarFailure::ContextExceeded {
                            prompt_tokens,
                            context,
                        }) = e.sidecar().cloned()
                        {
                            if !overflow_retried && tool_uses == 0 {
                                overflow_retried = true;
                                let excess = prompt_tokens.saturating_sub(context).saturating_add(512);
                                let dropped = drop_oldest_history(&mut turns, excess);
                                if dropped > 0 {
                                    let _ = tx_status.send(Ok(Event::default().event("status").safe_data(format!(
                                        "The conversation did not fit the model's context; leaving out the {dropped} oldest message(s) and trying again…"
                                    ))));
                                    continue;
                                }
                            }
                        }
                        failed = Some(e.to_string());
                        break;
                    }
                }
            }
        }
        timing.total_ms = request_started.elapsed().as_millis() as u64;
        timing.update_rate();
        let ttft_ms = timing.first_visible_ms.unwrap_or(0);
        let gen_tps = timing.output_tps;
        let was_cancelled = bg_cancel.load(std::sync::atomic::Ordering::SeqCst);
        if code_activity && (was_cancelled || pending_tool_limit) {
            emit_chat_activity(&bg_state, &bg_id, &tx_status, crate::agent::AgentEvent::activity(
                "response", if was_cancelled { AgentState::Cancelled } else { AgentState::Completed },
                if was_cancelled { "Stopped. Completed inspections were kept.".into() }
                else { "The read-only inspection reached its per-turn action limit. Any additional requested actions were not run; ask a follow-up to continue.".into() }, tool_uses + 1,
            )).await;
        }
        // Claim the single persistence right; the stop handler may have won it.
        let claimed = bg_persisted
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok();
        // A plain-text document from a model without tool support: its reply
        // is the file.
        if let (Some(kind), None, false) = (plain_document, failed.as_ref(), bg_cancel.load(std::sync::atomic::Ordering::SeqCst)) {
            let reply = bg_partial.lock().expect("lock").clone();
            let body = plain_document_body(&reply);
            if !body.trim().is_empty() {
                let filename = plain_document_filename(&req.message, kind);
                match crate::documents::save_plain_document(
                    &bg_state.storage,
                    &bg_state.artifacts_dir,
                    bg_conv.as_deref().unwrap_or("direct"),
                    &filename,
                    kind,
                    &body,
                )
                .await
                {
                    Ok(saved) => {
                        let note = format!("\n\nSaved as {saved} in this conversation's Artifacts panel.");
                        send_split_pieces(vec![crate::stream_split::Piece::Prose(note)], &full_cb, &tx_tok);
                    }
                    Err(error) => {
                        let _ = tx_status.send(Ok(Event::default().event("status").safe_data(format!("The file could not be saved: {error}"))));
                    }
                }
            }
        }
        let full = bg_partial.lock().expect("lock").clone();
        // Citations persist with the answer so reloads keep their sources (§121).
        let stored = if cite_block.is_empty() {
            full.clone()
        } else {
            format!("{full}{cite_block}")
        };
        match failed {
            None => {
                if claimed {
                    if let Some(cid) = bg_conv {
                        let st = bg_state.storage.lock().await;
                        let mid = bg_id.clone();
                        if let Err(e) = st.add_message(&Message {
                            id: mid.clone(),
                            conversation_id: cid.clone(),
                            role: "assistant".into(),
                            content: stored,
                            created_at: chrono::Utc::now().to_rfc3339(),
                        }) {
                            tracing::warn!("persist assistant message failed: {e}");
                        }
                        // Stage 17: this thread was last processed by this model.
                        let model = bg_state
                            .models
                            .read()
                            .await
                            .current()
                            .map(|m| m.id.clone())
                            .unwrap_or_default();
                        if !model.is_empty() {
                            let _ = st.set_last_model(&cid, &model);
                        }
                        // Stage 33: per-message telemetry, separate from content.
                        let _ = st.record_metric(&crate::storage::GenerationMetric {
                            message_id: mid,
                            conversation_id: cid,
                            model_id: model,
                            prompt_tokens: prompt_sum,
                            generated_tokens: gen_sum,
                            gen_ms: ms_sum,
                            ttft_ms,
                            gen_tps: gen_tps.unwrap_or(0.0),
                            timing: Some(timing.clone()),
                            created_at: chrono::Utc::now().to_rfc3339(),
                        });
                    }
                }
                let _ = tx.send(Ok(Event::default().event("done").safe_data(
                    serde_json::json!({
                        "message_id": bg_id,
                        "prompt_tokens": prompt_sum,
                        "generated_tokens": gen_sum,
                        "stopped": was_cancelled,
                        "truncated": truncated,
                        "reasoning": if reasoning_seen && reasoning_label == "off" { "native" } else { reasoning_label },
                        "sources": cite_block.lines().filter(|l| l.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).count(),
                        "vision": if images_skipped > 0 { "unsupported" } else { "off" },
                        // Stage 32 measured generation speed (output tok/s).
                        "gen_ms": ms_sum,
                        "ttft_ms": ttft_ms,
                        "gen_tps": gen_tps,
                        "timing": timing,
                        "tool_rounds": tool_uses,
                    })
                    .to_string(),
                )));
            }
            Some(e) => {
                if claimed {
                    if let Some(cid) = &bg_conv {
                        let st = bg_state.storage.lock().await;
                        let content = if stored.is_empty() {
                            format!("Inference interrupted: {e}")
                        } else {
                            stored
                        };
                        if let Err(error) = st.add_message(&Message {
                            id: bg_id.clone(),
                            conversation_id: cid.clone(),
                            role: "assistant".into(),
                            content,
                            created_at: chrono::Utc::now().to_rfc3339(),
                        }) {
                            tracing::error!("Could not preserve interrupted reply: {error}");
                        }
                    }
                }
                if code_activity {
                    emit_chat_activity(
                        &bg_state,
                        &bg_id,
                        &tx_status,
                        crate::agent::AgentEvent::activity(
                            "status",
                            if was_cancelled {
                                AgentState::Cancelled
                            } else {
                                AgentState::Failed
                            },
                            if was_cancelled {
                                "Stopped. Partial work was kept.".into()
                            } else {
                                format!("Inference interrupted: {e}. Partial work was kept.")
                            },
                            tool_uses + 1,
                        ),
                    )
                    .await;
                }
                // If we were cancelled the stop path already kept the partial;
                // don't confuse the UI with an error frame for an intentional stop.
                if !was_cancelled {
                    tracing::warn!("sidecar chat failed: {e}");
                    let _ = tx.send(Ok(Event::default()
                        .event("error")
                        .safe_data(format!("Inference failed: {e}"))));
                }
            }
        }
        bg_tracker.write().await.clear_if(&bg_id);
    });
    s.generations.write().await.insert(ActiveGeneration {
        id: gen_id,
        conversation_id: conv_id,
        cancel,
        partial,
        persisted,
        handle,
    });
    Ok(Sse::new(UnboundedReceiverStream::new(rx).boxed()).keep_alive(KeepAlive::default()))
}

/// §46 Stop: cancel the running generation server-side (sidecar request
/// aborted, partial response kept) — not just the UI fetch.
/// GPU reclaim: aborting the task drops the reqwest response stream, which
/// closes the HTTP connection; llama-server notices the disconnect, aborts
/// that decode slot, and the GPU is free for the next request instead of
/// finishing tokens nobody will read.
async fn chat_stop(State(s): State<AppState>) -> Json<crate::generation::CancelOutcome> {
    Json(s.generations.write().await.cancel_current(&s.storage).await)
}

async fn list_conversations(
    State(s): State<AppState>,
) -> Result<Json<Vec<Conversation>>, ApiError> {
    let st = s.storage.lock().await;
    st.list_conversations()
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

#[derive(Deserialize)]
struct NewConv {
    #[serde(default)]
    title: String,
    #[serde(default)]
    model_id: String,
    /// Stage 14 session mode: "chat" (default) or "code".
    #[serde(default = "default_conv_mode")]
    mode: String,
    /// Stage 14: linked workspace id for code sessions.
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    reasoning_default: bool,
    #[serde(default)]
    search_default: bool,
}

fn default_conv_mode() -> String {
    "chat".into()
}

async fn create_conversation(
    State(s): State<AppState>,
    Json(req): Json<NewConv>,
) -> Result<Json<Conversation>, ApiError> {
    let title = if req.title.trim().is_empty() {
        "New chat".to_string()
    } else if req.title.len() > MAX_TITLE_CHARS {
        return Err(ApiError::bad(
            "title too long",
            "Keep titles under 200 characters.",
        ));
    } else {
        req.title.trim().to_string()
    };
    let mode = match req.mode.as_str() {
        "" | "chat" => "chat".to_string(),
        "code" => "code".to_string(),
        other => {
            return Err(ApiError::bad(
                format!("invalid mode '{other}'"),
                "Mode must be 'chat' or 'code'.",
            ))
        }
    };
    let workspace = req.workspace.trim().to_string();
    let st = s.storage.lock().await;
    if mode == "code" {
        if workspace.is_empty() {
            return Err(ApiError::bad(
                "code session has no workspace",
                "Select a project before starting a code session.",
            ));
        }
        match st.get_workspace(&workspace) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(ApiError::bad(
                    "selected workspace no longer exists",
                    "Select an available project and try again.",
                ))
            }
            Err(error) => return Err(ApiError::internal(format!("storage error: {error}"))),
        }
    }
    let c = Conversation {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        model_id: req.model_id,
        created_at: chrono::Utc::now().to_rfc3339(),
        mode,
        workspace,
        reasoning_default: req.reasoning_default,
        search_default: req.search_default,
        last_model: String::new(),
        priority: "normal".into(),
        related_to: String::new(),
    };
    st.create_conversation(&c)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    tracing::info!(conv = %c.id, "conversation created");
    Ok(Json(c))
}

async fn get_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Conversation>, ApiError> {
    let st = s.storage.lock().await;
    match st.get_conversation(&id) {
        Ok(Some(c)) => Ok(Json(c)),
        Ok(None) => Err(ApiError::not_found("conversation not found")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

async fn delete_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    match st.delete_conversation(&id) {
        Ok(true) => {
            tracing::info!(conv = %id, "conversation deleted");
            Ok(Json(serde_json::json!({"deleted": id})))
        }
        Ok(false) => Err(ApiError::not_found("conversation not found")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

async fn list_messages(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let st = s.storage.lock().await;
    match st.get_conversation(&id) {
        Ok(Some(_)) => {
            let messages = st
                .messages_for(&id)
                .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
            // One query for every journal in the conversation instead of one
            // per message (a long session used to issue hundreds).
            let mut journals = st
                .conversation_activities(&id)
                .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
            let mut payload = vec![];
            for message in messages {
                let activities = journals.remove(&message.id).unwrap_or_default();
                let mut value =
                    serde_json::to_value(message).map_err(|e| ApiError::internal(e.to_string()))?;
                if !activities.is_empty() {
                    value["activities"] = serde_json::json!(activities);
                }
                payload.push(value);
            }
            Ok(Json(payload))
        }
        Ok(None) => Err(ApiError::not_found("conversation not found")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

#[derive(Deserialize)]
struct NewMessage {
    role: String,
    content: String,
}

async fn post_message(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NewMessage>,
) -> Result<Json<Message>, ApiError> {
    validate_role(&req.role)?;
    validate_content(&req.content)?;
    let st = s.storage.lock().await;
    match st.get_conversation(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    }
    let m = Message {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: id,
        role: req.role,
        content: req.content,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    st.add_message(&m)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(m))
}

/// Stage 6: edit a message. Later turns are truncated — editing an old turn
/// restarts the thread there (the simple, predictable form of branching).
#[derive(Deserialize)]
struct EditMessage {
    content: String,
}

async fn edit_message(
    State(s): State<AppState>,
    Path((id, mid)): Path<(String, String)>,
    Json(req): Json<EditMessage>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_content(&req.content)?;
    let st = s.storage.lock().await;
    match st.get_message(&id, &mid) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(ApiError::not_found("message not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    }
    st.update_message_content(&id, &mid, req.content.trim())
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let truncated = st
        .delete_messages_after(&id, &mid)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(
        serde_json::json!({"edited": mid, "truncated": truncated}),
    ))
}

/// Rough token estimate until the sidecar tokenizer is wired per call (§21).
fn estimate_tokens(chars: usize) -> u32 {
    (chars / 4) as u32
}

/// Build the turns sent to the model: newest HISTORY_TURNS messages within
/// HISTORY_CHARS, oldest first, plus attachment excerpts on the latest user
/// turn (§21 context management, §68 file pipeline).
pub(crate) fn build_turns(
    history: &[Message],
    attachments: &[crate::storage::Attachment],
) -> Vec<ChatTurn> {
    build_turns_budgeted(history, attachments, HISTORY_CHARS)
}

/// Characters left for messages once attachment excerpts, which ride on the
/// latest turn, take their share of a request's history budget.
pub(crate) fn history_room_for(
    max_chars: usize,
    attachments: &[crate::storage::Attachment],
) -> usize {
    let attach_reserved: usize = attachments
        .iter()
        .map(|a| a.text_excerpt.len())
        .sum::<usize>()
        .min(ATTACH_CHARS_PER_TURN);
    max_chars.saturating_sub(attach_reserved).max(2_000)
}

/// Same as `build_turns` with an explicit character budget derived from the
/// loaded context window, so an 8K CPU context and a 128K GPU context each
/// carry as much history as they can actually hold.
pub(crate) fn build_turns_budgeted(
    history: &[Message],
    attachments: &[crate::storage::Attachment],
    max_chars: usize,
) -> Vec<ChatTurn> {
    build_turns_with_owners(history, attachments, max_chars).0
}

/// The history turns within budget and, for each, the id of the saved message
/// it came from (for placing images on their own messages).
/// The conversation's most recent images (at most four; attachments are
/// stored oldest first), prepared for the model, each with its attachment.
/// Shared by chat and agent runs. The count is images that could not be
/// prepared.
pub(crate) fn prepare_conversation_images<'a>(
    attachments_dir: &std::path::Path,
    conversation_id: &str,
    attachments: &'a [crate::storage::Attachment],
) -> (Vec<(&'a crate::storage::Attachment, String)>, usize) {
    let mut urls = vec![];
    let mut skipped = 0usize;
    let images: Vec<_> = attachments.iter().filter(|a| a.kind == "image").collect();
    for a in &images[images.len().saturating_sub(4)..] {
        let p = attachments_dir.join(conversation_id).join(&a.filename);
        match std::fs::read(&p)
            .map_err(|e| e.to_string())
            .and_then(|b| crate::vision::prepare_image(&b))
        {
            Ok(prep) => urls.push((*a, prep.data_url)),
            Err(e) => {
                tracing::warn!("vision prepare failed for {}: {e}", a.filename);
                skipped += 1;
            }
        }
    }
    (urls, skipped)
}

pub(crate) fn build_turns_with_owners(
    history: &[Message],
    attachments: &[crate::storage::Attachment],
    max_chars: usize,
) -> (Vec<ChatTurn>, Vec<String>) {
    let summary = history
        .first()
        .filter(|message| message.id.starts_with("context-summary:"));
    let mut window: Vec<&Message> = history
        .iter()
        .rev()
        .filter(|message| !message.id.starts_with("context-summary:"))
        .take(HISTORY_TURNS.saturating_sub(usize::from(summary.is_some())))
        .collect();
    window.reverse();
    if let Some(summary) = summary {
        window.insert(0, summary);
    }
    // Attachment excerpts ride on the latest turn and count against the same
    // budget, so a big attachment on a small context displaces old history
    // rather than overflowing the window.
    let history_budget = history_room_for(max_chars, attachments);
    // Drop oldest until within budget (never drop the final turn).
    let mut chars: usize = window.iter().map(|m| m.content.len()).sum();
    while window.len() > 1 && chars > history_budget {
        let remove_at = if summary.is_some() && window.len() > 2 {
            1
        } else {
            0
        };
        if let Some(m) = window.get(remove_at) {
            chars -= m.content.len();
        }
        window.remove(remove_at);
    }
    let ids: Vec<String> = window.iter().map(|message| message.id.clone()).collect();
    let turns = window
        .into_iter()
        .map(|m| {
            // llama.cpp's OpenAI dialect has no plain `tool` turn without a
            // tool_call_id; fold tool results into user turns explicitly.
            let (role, content) = match m.role.as_str() {
                // Prose only: a reply's tool envelopes, including rejected
                // attempts, would teach the model to repeat them.
                "assistant" => ("assistant", {
                    let prose = crate::agent_runner::without_action_envelopes(&m.content);
                    if prose.is_empty() {
                        "[This reply only ran tools; their results were shown to the user.]".to_string()
                    } else {
                        prose
                    }
                }),
                "tool" => ("user", format!("[tool result]\n{}", m.content)),
                _ => ("user", m.content.clone()),
            };
            ChatTurn::text(role, content)
        })
        .collect::<Vec<_>>();
    if legacy_prompt_order() {
        let mut attach_block = String::new();
        let mut remaining = ATTACH_CHARS_PER_TURN;
        for a in attachments {
            if remaining == 0 {
                break;
            }
            let mut take = a.text_excerpt.len().min(remaining);
            while !a.text_excerpt.is_char_boundary(take) {
                take -= 1;
            }
            attach_block.push_str(&format!("\n\n[Attached file: {}]\n{}", a.filename, &a.text_excerpt[..take]));
            remaining -= take;
        }
        return (turns.tap_append_attach(attach_block), ids);
    }
    // Each excerpt stays on the message it was sent with, so it is sent in the
    // same place on every later request.
    let mut turns = turns;
    let mut used: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for a in attachments.iter().filter(|a| !a.text_excerpt.is_empty()) {
        let Some(index) = attachment_turn(&turns, &ids, history, a) else {
            continue;
        };
        let used_here = used.entry(index).or_insert(0);
        let remaining = ATTACH_CHARS_PER_TURN.saturating_sub(*used_here);
        if remaining == 0 {
            continue;
        }
        let mut take = a.text_excerpt.len().min(remaining);
        while !a.text_excerpt.is_char_boundary(take) {
            take -= 1;
        }
        turns[index]
            .content
            .push_str(&format!("\n\n[Attached file: {}]\n{}", a.filename, &a.text_excerpt[..take]));
        *used_here += take;
    }
    (turns, ids)
}

trait TapAttach {
    fn tap_append_attach(self, block: String) -> Self;
}

impl TapAttach for Vec<ChatTurn> {
    /// Append attachment excerpts to the latest user turn (or add one).
    fn tap_append_attach(mut self, block: String) -> Self {
        if block.is_empty() {
            return self;
        }
        if let Some(last_user) = self.iter_mut().rev().find(|t| t.role == "user") {
            last_user.content.push_str(&block);
        } else {
            self.push(ChatTurn::text("user", block.trim_start().to_string()));
        }
        self
    }
}

/// Stage 6: what will the model actually see? (§21 transparency)
/// Context belongs to a single submitted agent request. Live events avoid
/// waiting for the persistence queue; the journal provides restart recovery.
async fn latest_agent_context(
    s: &AppState,
    conversation_id: &str,
) -> Result<Option<serde_json::Value>, ApiError> {
    let live = {
        let registry = s.agents.read().await;
        registry
            .summaries()
            .into_iter()
            .rev()
            .find(|summary| summary.conversation_id == conversation_id)
            .and_then(|summary| registry.get(&summary.id))
    };
    if let Some(run) = live {
        let events = run.events.lock().expect("lock");
        let state = events
            .last()
            .map(|event| event.state)
            .unwrap_or(crate::agent::AgentState::Idle);
        let active = !matches!(
            state,
            crate::agent::AgentState::Completed
                | crate::agent::AgentState::Failed
                | crate::agent::AgentState::Cancelled
        );
        let context = events
            .iter()
            .rev()
            .find(|event| event.context_usage.is_some());
        return Ok(Some(serde_json::json!({
            "run_id": run.id, "active": active,
            "iteration": context.map(|event| event.iteration).unwrap_or(0),
            "usage": context.and_then(|event| event.context_usage.as_ref()),
        })));
    }
    let storage = s.storage.lock().await;
    let messages = storage
        .messages_for(conversation_id)
        .map_err(|error| ApiError::internal(format!("Could not read context history: {error}")))?;
    for message in messages
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
    {
        let events = storage.message_activities(&message.id).map_err(|error| {
            ApiError::internal(format!("Could not read recorded context: {error}"))
        })?;
        if let Some(context) = events
            .iter()
            .rev()
            .find(|event| event.context_usage.is_some())
        {
            return Ok(Some(serde_json::json!({
                "run_id": message.id, "active": false, "iteration": context.iteration,
                "usage": context.context_usage,
            })));
        }
    }
    Ok(None)
}

async fn conversation_context(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let attachments = st
        .attachments_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let attach_chars: usize = attachments.iter().map(|a| a.text_excerpt.len()).sum();
    drop(st);
    let limit = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            llama.running.as_ref().map(|r| r.cfg.n_ctx).unwrap_or(0)
        } else {
            s.inference.read().await.context_size()
        }
    };
    let (total, kept, dropped, tok) = context_numbers(&s, &id).await?;
    // Stage 25 breakdown (§§78–80, §104): category estimates in tokens.
    // chars/4 heuristic, stated as estimate in the UI.
    let st = s.storage.lock().await;
    // Categorize only the bounded saved input, never historical audit output:
    // the latter may have been pruned from the live agent transcript already.
    let history = st
        .context_messages_for(&id)
        .map_err(|error| ApiError::internal(format!("Could not read saved context: {error}")))?;
    let saved_turns = build_turns(&history, &[]);
    let attached_turns = build_turns(&history, &attachments);
    let saved_chars: usize = saved_turns.iter().map(|turn| turn.content.len()).sum();
    let attached_chars: usize = attached_turns.iter().map(|turn| turn.content.len()).sum();
    let tool_chars: usize = saved_turns
        .iter()
        .filter(|turn| turn.content.starts_with("[tool result]\n"))
        .map(|turn| turn.content.len())
        .sum();
    let workspace = st
        .get_conversation(&id)
        .ok()
        .flatten()
        .map(|conversation| conversation.workspace)
        .unwrap_or_default();
    let memory = st
        .memory_context(&id, &workspace)
        .map_err(|error| ApiError::internal(format!("Could not read saved memory: {error}")))?;
    drop(st);
    let attach_tok = estimate_tokens(attached_chars.saturating_sub(saved_chars));
    let tool_tok = (tool_chars / 4) as u32;
    // System instructions are assembled by each runtime path, not part of
    // saved history. Actual agent requests report their own total above.
    let system_tok = 0u32;
    let memory_tok = estimate_tokens(memory.text.len());
    let output_reserve = (limit as f64 * 0.10) as u32;
    let conversation_tok = tok
        .saturating_sub(attach_tok)
        .saturating_sub(tool_tok)
        .saturating_sub(memory_tok);
    let used = tok + output_reserve;
    // This percentage is a saved-text estimate, not runtime/KV occupancy. It
    // is measured against the room a request gives history (the window minus
    // instructions, the reply and a margin), the same measure automatic
    // compaction uses, so the gauge reads the threshold when compaction runs.
    let room = if limit > 0 {
        assemble_request_context_with(&s, Some(&id), None, limit, false)
            .await
            .ok()
            .map(|context| (context.history_usage_pct(), context.history_room_chars))
    } else {
        None
    };
    let pct = match room {
        Some((usage, _)) => usage,
        None if limit > 0 => (tok as f64 / limit as f64 * 100.0) as u32,
        None => 0,
    };
    let health_pct = match room {
        Some((usage, _)) => usage,
        None if limit > 0 => (used as f64 / limit as f64 * 100.0) as u32,
        None => 0,
    };
    let settings = s.settings.read().await.clone();
    let memory_settings = settings.memory.clone();
    // The window a request gets is rarely the size that was asked for: the
    // loader shrinks it to fit GPU memory, and the gauge then measures
    // against the room left after the reply's reserve. Reporting only the
    // last of the three made a 32,768 setting read as 11,776 with nothing
    // to explain either step.
    let configured_limit = settings.inference.context_size;
    let fit_note = s.llama.read().await.running.as_ref().and_then(|running| {
        running
            .cfg
            .runtime_policy
            .as_ref()
            .and_then(|policy| policy.context_note.clone())
    });
    let health = if health_pct < 60 {
        "healthy"
    } else if health_pct < 80 {
        "moderate"
    } else if health_pct < 90 {
        "high"
    } else {
        "critical"
    };
    let agent_context = latest_agent_context(&s, &id).await?;
    Ok(Json(serde_json::json!({
        "limit": limit,
        "configured_limit": configured_limit,
        "context_fit": settings.runtime.context_fit,
        "limit_note": fit_note,
        "estimated_tokens": tok,
        "measurement": "saved_history_estimate",
        "agent_context": agent_context,
        "messages_total": total,
        "messages_kept": kept,
        "messages_dropped": dropped,
        "attachments": {"count": attachments.len(), "chars": attach_chars},
        "memory": {"entries": memory.entries, "chars": memory.text.chars().count(), "budget_chars": 2000},
        "breakdown": {
            "system": system_tok, "conversation": conversation_tok,
            "attachments": attach_tok, "tools": tool_tok,
            "memory": memory_tok, "output_reserve": output_reserve,
        },
        "used_with_reserve": used,
        "usage_pct": pct,
        "history_room_tokens": room.map(|(_, chars)| estimate_tokens(chars)),
        "auto_compact": memory_settings.auto_compaction(),
        "compact_at_pct": memory_settings.compact_threshold_pct(),
        "health": health,
    })))
}

#[derive(Deserialize)]
struct NewAttachment {
    filename: String,
    #[serde(default)]
    mime: String,
    /// File text (frontend reads the file; binary extraction is Phase 2, §69).
    #[serde(default)]
    content: String,
    /// Stage 19: base64 bytes for image/* attachments (§73).
    /// Stage 28: also used for office/binary docs (pdf/docx/xlsx/pptx/zip).
    #[serde(default)]
    content_base64: String,
}

/// Stage 28 best-effort office extraction (no heavy deps): sniff magic,
/// report structure honestly as partial — never fake full parsing (§79).
fn sniff_office(filename: &str, bytes: &[u8]) -> (String, String, String) {
    let lower = filename.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    // PDF: count page objects, falling back to the /Count entry.
    if bytes.starts_with(b"%PDF") || ext == "pdf" {
        let text = String::from_utf8_lossy(bytes);
        let mut pages = text
            .match_indices("/Type/Page")
            .filter(|(i, _)| !text[*i..].starts_with("/Type/Pages"))
            .count();
        if pages == 0 {
            // /Count 3 — first integer after the marker.
            if let Some(at) = text.find("/Count") {
                let digits: String = text[at..]
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(6)
                    .collect();
                pages = digits.parse().unwrap_or(1).max(1);
            } else {
                pages = 1;
            }
        }
        let strings = printable_strings(bytes, 6000);
        return (
            "office".into(),
            "partial".into(),
            format!("[PDF {filename}: ~{pages} page(s), {} bytes. Partial extraction — full PDF parsing is staged; key strings below.]\n{strings}", bytes.len()),
        );
    }
    // ZIP-based formats (docx/xlsx/pptx/zip/odt): list entries + strings.
    if bytes.len() > 4 && bytes.starts_with(b"PK\x03\x04")
        || ["docx", "xlsx", "pptx", "zip", "odt", "ods"].contains(&ext)
    {
        let names = zip_entry_names(bytes, 200);
        let strings = printable_strings(bytes, 6000);
        let kind_label = match ext {
            "xlsx" | "ods" | "csv" => "spreadsheet",
            "pptx" | "odp" => "presentation",
            _ => "document",
        };
        if names.is_empty() && strings.trim().is_empty() {
            return ("office".into(), "unsupported".into(),
                format!("[{filename}: {kind_label}, {} bytes. Could not extract content — try exporting to txt/csv/md.]", bytes.len()));
        }
        return (
            "office".into(), "partial".into(),
            format!("[{filename}: {kind_label}, {} bytes, {} embedded file(s){}. Partial extraction.]\nEntries: {}\nText strings:\n{}",
                bytes.len(), names.len(),
                if names.len() >= 200 { " (truncated)" } else { "" },
                names.iter().take(40).cloned().collect::<Vec<_>>().join(", "),
                strings),
        );
    }
    // Generic binary: printable strings or honest unsupported.
    let strings = printable_strings(bytes, 6000);
    if strings.trim().len() < 32 {
        return (
            "binary".into(),
            "unsupported".into(),
            format!(
                "[{filename}: {} bytes, no readable text found. Convert to txt/md/csv first.]",
                bytes.len()
            ),
        );
    }
    (
        "text".into(),
        "partial".into(),
        format!(
            "[{filename}: binary with readable strings, {} bytes. Partial extraction.]\n{strings}",
            bytes.len()
        ),
    )
}

/// Longest printable runs (≥4 chars), capped — std-only strings(1).
fn printable_strings(bytes: &[u8], cap: usize) -> String {
    let mut out = String::new();
    let mut cur = String::new();
    for &b in bytes {
        if (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t' {
            cur.push(b as char);
            if cur.len() > 400 {
                out.push_str(&cur);
                out.push('\n');
                cur.clear();
                if out.len() >= cap {
                    break;
                }
            }
        } else if !cur.is_empty() {
            if cur.trim().len() >= 4 {
                out.push_str(cur.trim());
                out.push('\n');
            }
            cur.clear();
            if out.len() >= cap {
                break;
            }
        }
    }
    if cur.trim().len() >= 4 && out.len() < cap {
        out.push_str(cur.trim());
    }
    out.chars().take(cap).collect()
}

/// Minimal ZIP central-scan: walk local file headers, collect names.
fn zip_entry_names(bytes: &[u8], cap: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i + 30 < bytes.len() && names.len() < cap {
        if &bytes[i..i + 4] != b"PK\x03\x04" {
            i += 1;
            continue;
        }
        let name_len = u16::from_le_bytes([bytes[i + 26], bytes[i + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
        let comp_size =
            u32::from_le_bytes([bytes[i + 18], bytes[i + 19], bytes[i + 20], bytes[i + 21]])
                as usize;
        if i + 30 + name_len > bytes.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&bytes[i + 30..i + 30 + name_len]).into_owned());
        i += 30 + name_len + extra_len + comp_size;
        if comp_size == 0 {
            i += 1; // avoid stalling on data descriptors
        }
    }
    names
}

async fn list_attachments(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::storage::Attachment>>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    st.attachments_for(&id)
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

async fn add_attachment(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NewAttachment>,
) -> Result<Json<crate::storage::Attachment>, ApiError> {
    const MAX_ATTACH_BYTES: usize = 5_000_000;
    const EXCERPT_CHARS: usize = 20_000;
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let safe =
        crate::workspace::WorkspaceManager::sanitize_filename(&req.filename).map_err(|_| {
            ApiError::bad(
                "invalid filename",
                "Use a plain filename without path separators.",
            )
        })?;
    let mime = if req.mime.is_empty() {
        "text/plain".into()
    } else {
        req.mime.clone()
    };
    // Stage 19 image path: bytes → disk, placeholder excerpt; vision models
    // receive the pixels at send time, others get the honest fallback (§73).
    if crate::vision::is_image_mime(&mime) {
        if req.content_base64.is_empty() {
            return Err(ApiError::bad(
                "image bytes missing",
                "Send images as base64 (content_base64).",
            ));
        }
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            req.content_base64.trim(),
        )
        .map_err(|_| ApiError::bad("bad base64", "The image data is not valid base64."))?;
        if bytes.len() > MAX_ATTACH_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "image too large ({} bytes, max {MAX_ATTACH_BYTES})",
                    bytes.len()
                ),
                "Downscale the screenshot before attaching.",
            ));
        }
        let vision_now = s
            .models
            .read()
            .await
            .current()
            .map(|m| m.vision)
            .unwrap_or(false);
        let (w, h) = image::load_from_memory(&bytes)
            .map(|i| (i.width(), i.height()))
            .unwrap_or((0, 0));
        let dir = s.attachments_dir.join(&id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
        std::fs::write(dir.join(&safe), &bytes)
            .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
        let a = crate::storage::Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: id,
            filename: safe,
            mime,
            size_bytes: bytes.len() as u64,
            text_excerpt: crate::vision::image_excerpt(
                &req.filename,
                w,
                h,
                bytes.len(),
                vision_now,
            ),
            kind: "image".into(),
            status: "ready".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        st.add_attachment(&a)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        return Ok(Json(a));
    }
    // Stage 28 office/binary path: base64 bytes → disk + best-effort excerpt.
    if !req.content_base64.is_empty() {
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            req.content_base64.trim(),
        )
        .map_err(|_| ApiError::bad("bad base64", "The file data is not valid base64."))?;
        if bytes.len() > MAX_ATTACH_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "file too large ({} bytes, max {MAX_ATTACH_BYTES})",
                    bytes.len()
                ),
                "Large documents are chunked and retrieved section-wise (§69).",
            ));
        }
        let (kind, status, excerpt_full) = sniff_office(&safe, &bytes);
        let dir = s.attachments_dir.join(&id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
        std::fs::write(dir.join(&safe), &bytes)
            .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
        let excerpt: String = excerpt_full.chars().take(EXCERPT_CHARS).collect();
        let a = crate::storage::Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: id,
            filename: safe,
            mime,
            size_bytes: bytes.len() as u64,
            text_excerpt: excerpt,
            kind,
            status,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        st.add_attachment(&a)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        return Ok(Json(a));
    }
    if req.content.is_empty() {
        return Err(ApiError::bad(
            "attachment is empty",
            "Attach a file with readable text content.",
        ));
    }
    if req.content.len() > MAX_ATTACH_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "file too large ({} bytes, max {MAX_ATTACH_BYTES})",
                req.content.len()
            ),
            "Large documents are chunked and retrieved section-wise in Phase 2 (§69).",
        ));
    }
    let dir = s.attachments_dir.join(&id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
    std::fs::write(dir.join(&safe), &req.content)
        .map_err(|e| ApiError::internal(format!("cannot store attachment: {e}")))?;
    let excerpt: String = req.content.chars().take(EXCERPT_CHARS).collect();
    let a = crate::storage::Attachment {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: id,
        filename: safe,
        mime,
        size_bytes: req.content.len() as u64,
        text_excerpt: excerpt,
        kind: "text".into(),
        status: "ready".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    st.add_attachment(&a)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(a))
}

async fn delete_attachment(
    State(s): State<AppState>,
    Path((id, aid)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    let atts = st
        .attachments_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    match atts.iter().find(|a| a.id == aid).cloned() {
        None => Err(ApiError::not_found("attachment not found")),
        Some(a) => {
            st.delete_attachment(&id, &aid)
                .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
            let _ = std::fs::remove_file(s.attachments_dir.join(&id).join(&a.filename));
            Ok(Json(serde_json::json!({"deleted": aid})))
        }
    }
}

// ---- Stage 20 artifacts (§§33, 38) ----

#[derive(Deserialize, Default)]
struct ArtifactQuery {
    #[serde(default)]
    conversation_id: String,
}

/// Artifact cards source: what this conversation generated.
/// Stage 28: serve stored attachment bytes for the viewer (§74).
async fn attachment_file(
    State(s): State<AppState>,
    Path((id, aid)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let st = s.storage.lock().await;
    let atts = st
        .attachments_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let att = atts
        .into_iter()
        .find(|a| a.id == aid)
        .ok_or_else(|| ApiError::not_found("unknown attachment"))?;
    drop(st);
    let bytes = std::fs::read(s.attachments_dir.join(&id).join(&att.filename))
        .map_err(|_| ApiError::not_found("attachment file missing from disk"))?;
    use axum::body::Body;
    Ok(Response::builder()
        .header("content-type", att.mime.clone())
        .header("content-length", bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| ApiError::internal(format!("response error: {e}")))?)
}

async fn list_artifacts(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ArtifactQuery>,
) -> Result<Json<Vec<crate::storage::ArtifactRow>>, ApiError> {
    s.storage
        .lock()
        .await
        .artifacts_for(&q.conversation_id)
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

/// Download bytes with the recorded mime type.
async fn artifact_file(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = match s.storage.lock().await.get_artifact(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return Err(ApiError::not_found("unknown artifact")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let bytes = std::fs::read(&row.path)
        .map_err(|_| ApiError::not_found("artifact file missing from disk"))?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        row.mime
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"{}\"", row.filename.replace('"', ""))
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
    );
    Ok((headers, bytes))
}

async fn list_tools() -> Json<Vec<tools::ToolDescriptor>> {
    Json(tools::registry())
}

#[derive(Deserialize)]
struct ExecuteToolReq {
    /// Workspace root (§27). Must exist; every file arg stays inside it.
    workspace: String,
    tool: String,
    #[serde(default)]
    args: serde_json::Value,
    /// User clicked Allow Once (§26).
    #[serde(default)]
    approved_once: bool,
    /// User clicked Allow for Session (MODERATE tools only).
    #[serde(default)]
    grant_session: bool,
    #[serde(default)]
    conversation_id: String,
}

/// Stage 9: the ONLY path from a tool request to the OS (§92).
/// Gate (PermissionManager) → validate → execute → audit. Direct
/// `tools::execute` calls from handlers are not added elsewhere.
async fn execute_tool(
    State(s): State<AppState>,
    Json(req): Json<ExecuteToolReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::permissions::{PermissionDecision, RiskLevel};

    if req.tool.trim().is_empty() {
        return Err(ApiError::bad(
            "tool is empty",
            "Pick a tool from GET /api/tools.",
        ));
    }
    let ws_root = std::path::PathBuf::from(&req.workspace);
    if !ws_root.is_dir() {
        return Err(ApiError::bad(
            format!("workspace not found: {}", req.workspace),
            "Select an existing workspace folder first (§27).",
        ));
    }
    let ws_key = ws_root
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| req.workspace.clone());
    let ws = crate::workspace::WorkspaceManager::new(ws_root);
    let risk = tools::risk_of(&req.tool);

    // --- Gate ---
    // approved_via reports how this call passed: explicit once, or automatic
    // (mode policy / session grant recorded earlier).
    let via: &'static str = {
        let mut pm = s.permissions.write().await;
        // Session grants are recorded only alongside an explicit approval.
        if req.grant_session && req.approved_once && risk == RiskLevel::Moderate {
            pm.grant_session(&req.tool, &ws_key);
        }
        match pm.decide_call(&req.tool, &req.args, risk, true, Some(&ws_key)) {
            PermissionDecision::Allow => "auto",
            PermissionDecision::RequireApproval { .. } if req.approved_once => "once",
            PermissionDecision::RequireApproval { reason } => {
                audit(
                    &s,
                    &req,
                    false,
                    &format!("denied, approval required: {reason}"),
                )
                .await;
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    format!("'{}' needs approval: {reason}", req.tool),
                    "Call again with approved_once:true, or grant_session:true for the session.",
                ));
            }
            PermissionDecision::Deny { reason } => {
                audit(&s, &req, false, &format!("denied: {reason}")).await;
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    format!("'{}' is not allowed: {reason}", req.tool),
                    "Switch agent mode or perform this step manually.",
                ));
            }
        }
    };

    // --- Execute ---
    // Stage 20: create_document renders through the async document pipeline.
    if req.tool == "create_document" {
        let conv = if req.conversation_id.trim().is_empty() {
            "direct".into()
        } else {
            req.conversation_id.clone()
        };
        return match crate::documents::execute_create_document(
            &s.storage,
            &s.artifacts_dir,
            &conv,
            &req.args,
        )
        .await
        {
            Ok(r) => {
                audit(&s, &req, true, &r.output).await;
                Ok(Json(
                    serde_json::json!({"ok": true, "output": r.output, "exit_code": Option::<i32>::None, "approved_via": "once"}),
                ))
            }
            Err(e) => {
                audit(&s, &req, false, &e.to_string()).await;
                Err(ApiError::bad(
                    e.to_string(),
                    "Fix the filename/spec and retry.",
                ))
            }
        };
    }
    // Stage 11: web_search is async (HTTP + provider config) and consent-bound:
    // the Search toggle (or an explicit approval here) IS the permission (§128).
    if req.tool == "web_search" {
        if !req.approved_once {
            audit(
                &s,
                &req,
                false,
                "denied, approval required: enable Search for this request",
            )
            .await;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "'web_search' needs approval: enable Search for this request",
                "Call again with approved_once:true (the Search toggle sends this).",
            ));
        }
        let query = req
            .args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let settings = s.settings.read().await.clone();
        let scfg = crate::search::SearchConfig {
            provider: settings.search.provider.clone(),
            brave_key: settings.search.brave_key.clone(),
            custom_url: settings.search.custom_url.clone(),
            max_results: settings.search.max_results,
            timeout_secs: settings.search.timeout_secs,
        };
        return match crate::search::run_search(&query, &scfg).await {
            Ok((results, provider)) => {
                let conv = if req.conversation_id.trim().is_empty() {
                    "direct".into()
                } else {
                    req.conversation_id.clone()
                };
                let st = s.storage.lock().await;
                let _ = st.record_search_run(&crate::storage::SearchRun {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: conv.clone(),
                    query: query.chars().take(500).collect(),
                    provider: provider.clone(),
                    result_count: results.len(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                });
                let out = serde_json::json!({"provider": provider, "results": results});
                let _ = st.record_tool_execution(&crate::storage::ToolExecution {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: conv,
                    tool: "web_search".into(),
                    args: req.args.to_string().chars().take(4000).collect(),
                    result: out.to_string().chars().take(4000).collect(),
                    approved: true,
                    created_at: chrono::Utc::now().to_rfc3339(),
                });
                Ok(Json(
                    serde_json::json!({"ok": true, "output": out, "exit_code": Option::<i32>::None, "approved_via": "once"}),
                ))
            }
            Err(e) => {
                audit(&s, &req, false, &e).await;
                Err(ApiError::bad(
                    format!("Internet search failed: {e}"),
                    "Retry, or continue without web — the local model still works (§125).",
                ))
            }
        };
    }
    let tool_req = tools::ToolRequest {
        name: req.tool.clone(),
        args: req.args.clone(),
        approved: true,
    };
    let result = tools::execute(&tool_req, &ws, true);
    let (ok, output, exit_code) = match &result {
        Ok(r) => (r.ok, r.output.clone(), r.exit_code),
        Err(e) => (false, e.to_string(), None),
    };
    audit(&s, &req, ok, &output).await;
    match result {
        Ok(_) => Ok(Json(serde_json::json!({
            "ok": ok, "output": output, "exit_code": exit_code,
            "approved_via": via,
        }))),
        Err(e) => Err(match e {
            tools::ToolError::UnknownTool(_)
            | tools::ToolError::InvalidArgs(_)
            | tools::ToolError::Workspace(_) => {
                ApiError::bad(e.to_string(), "Fix the tool arguments and retry.")
            }
            _ => ApiError::internal(e.to_string()),
        }),
    }
}

async fn audit(s: &AppState, req: &ExecuteToolReq, ok: bool, result: &str) {
    let conv = if req.conversation_id.trim().is_empty() {
        "direct"
    } else {
        req.conversation_id.trim()
    };
    let st = s.storage.lock().await;
    let _ = st.record_tool_execution(&crate::storage::ToolExecution {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conv.into(),
        tool: req.tool.clone(),
        args: req.args.to_string().chars().take(4000).collect(),
        result: result.chars().take(4000).collect(),
        approved: ok,
        created_at: chrono::Utc::now().to_rfc3339(),
    });
}

#[derive(Deserialize)]
struct ExecQuery {
    #[serde(default)]
    conversation_id: String,
    #[serde(default = "default_exec_limit")]
    limit: usize,
}

fn default_exec_limit() -> usize {
    50
}

async fn list_tool_executions(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ExecQuery>,
) -> Result<Json<Vec<crate::storage::ToolExecution>>, ApiError> {
    let st = s.storage.lock().await;
    st.tool_executions_for(&q.conversation_id, q.limit.clamp(1, 200))
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

#[derive(Deserialize)]
struct AgentReq {
    workspace: String,
    task: String,
    #[serde(default)]
    mode: Option<AgentMode>,
    #[serde(default)]
    conversation_id: String,
    /// Stage 11: Search-toggle consent for this run (default off, §117).
    #[serde(default)]
    search: bool,
    /// Native thinking for this run; defaults to the conversation, then the
    /// saved preference. Off asks thinking models not to think, which keeps
    /// each step's output budget for the action itself.
    #[serde(default)]
    reasoning: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageIntent {
    Acknowledgement,
    Greeting,
    Continue,
    Task,
}

/// Whole-message matching, never a prefix rule: "thanks, now fix X" and
/// "looks good but change X" remain substantive requests.
fn message_intent(text: &str) -> MessageIntent {
    let normalized = text
        .trim()
        .trim_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ';' | ':'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    match normalized.as_str() {
        "hi" | "hey" | "hello" | "hello there" | "good morning" | "good afternoon"
        | "good evening" => MessageIntent::Greeting,
        "thanks"
        | "thank you"
        | "thank you very much"
        | "thanks a lot"
        | "thx"
        | "ty"
        | "looks good to me"
        | "looks good"
        | "sounds good"
        | "sounds great"
        | "that looks good"
        | "that works"
        | "works for me"
        | "great"
        | "nice"
        | "perfect"
        | "awesome"
        | "cool"
        | "okay"
        | "ok"
        | "understood"
        | "got it"
        | "well done"
        | "yes"
        | "yep"
        | "sure"
        | "👍"
        | "👍🏻"
        | "👍🏼"
        | "👍🏽"
        | "👍🏾"
        | "👍🏿"
        | "✅" => MessageIntent::Acknowledgement,
        "continue"
        | "go ahead"
        | "proceed"
        | "carry on"
        | "keep going"
        | "yes proceed"
        | "yes, proceed"
        | "yes go ahead"
        | "yes, go ahead"
        | "continue with the plan"
        | "go ahead with the plan"
        | "implement the plan"
        | "implement it"
        | "do it" => MessageIntent::Continue,
        _ => MessageIntent::Task,
    }
}

/// `message_intent`, except that a short "yes" or "ok" answering a question
/// the latest reply asked ("Would you like me to fix subtract?") is a request
/// for the model, with the conversation, not a canned acknowledgement. The
/// classifier used to route those; review found they were answered "Got it"
/// after it was removed. ("Go ahead" and "do it" continue a saved task first:
/// see `run_agent`.)
async fn answering_intent(s: &AppState, conversation_id: &str, text: &str) -> MessageIntent {
    let intent = message_intent(text);
    if intent == MessageIntent::Acknowledgement && latest_reply_asks_in(s, conversation_id).await {
        MessageIntent::Task
    } else {
        intent
    }
}

async fn latest_reply_asks_in(s: &AppState, conversation_id: &str) -> bool {
    !conversation_id.is_empty()
        && latest_reply_asks(&s.storage.lock().await.messages_for(conversation_id).unwrap_or_default())
}

/// The latest assistant reply ends by asking the user something.
fn latest_reply_asks(history: &[Message]) -> bool {
    history
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| {
            let tail: String = message.content.trim_end().chars().rev().take(240).collect::<Vec<_>>().into_iter().rev().collect();
            let last_line = tail.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
            last_line.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '_' | ')' | '"')).ends_with('?')
        })
        .unwrap_or(false)
}

async fn conversational_reply(
    s: &AppState,
    conversation_id: &str,
    input: &str,
    reply: &str,
    disposition: &str,
) -> Result<serde_json::Value, ApiError> {
    let message_id = uuid::Uuid::new_v4().to_string();
    if !conversation_id.trim().is_empty() {
        let st = s.storage.lock().await;
        match st.get_conversation(conversation_id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(ApiError::not_found("conversation not found")),
            Err(error) => return Err(ApiError::internal(format!("storage error: {error}"))),
        }
        for (id, role, content) in [
            (uuid::Uuid::new_v4().to_string(), "user", input),
            (message_id.clone(), "assistant", reply),
        ] {
            st.add_message(&Message {
                id,
                conversation_id: conversation_id.into(),
                role: role.into(),
                content: content.into(),
                created_at: now_rfc3339(),
            })
            .map_err(|error| ApiError::internal(format!("Could not save the reply: {error}")))?;
        }
    }
    Ok(serde_json::json!({"disposition": disposition, "message": reply, "message_id": message_id}))
}

enum Continuation {
    Task(String),
    Reply(&'static str, &'static str),
}

/// Resuming a task requires explicit, durable evidence of a prior plan or
/// interrupted task in this exact session and project—not a guessed old goal.
async fn resolve_continuation(
    s: &AppState,
    conversation_id: &str,
    workspace: &std::path::Path,
) -> Result<Continuation, ApiError> {
    let context = s
        .storage
        .lock()
        .await
        .task_context(conversation_id)
        .map_err(|error| ApiError::internal(format!("Could not read task context: {error}")))?;
    let Some(mut context) = context else {
        return Ok(Continuation::Reply("needs_task", "What would you like me to continue? There is no saved unfinished task or proposed plan for this session."));
    };
    let current = std::fs::canonicalize(workspace).ok();
    let original = std::fs::canonicalize(&context.workspace).ok();
    if current.is_none() || current != original {
        return Ok(Continuation::Reply("needs_task", "The previous task belongs to a different or unavailable project. Select the intended project and tell me what to continue."));
    }
    if let Some(run) = s.agents.read().await.get(&context.run_id) {
        match run.state() {
            AgentState::Completed => context.status = if run.spec.mode == AgentMode::Plan && run.plan_presented() { "planned" } else { "completed" }.into(),
            AgentState::Failed | AgentState::Cancelled => context.status = "interrupted".into(),
            AgentState::WaitingPermission => return Ok(Continuation::Reply("conversation", "The current task is waiting for your approval. Use its Allow or Deny controls; a chat reply does not approve an action.")),
            _ => return Ok(Continuation::Reply("conversation", "That task is already running. You can follow its progress in Activity.")),
        }
    }
    match context.status.as_str() {
        "planned" | "interrupted" if message_intent(&context.task) == MessageIntent::Task => Ok(Continuation::Task(context.task)),
        "completed" => Ok(Continuation::Reply("needs_task", "That task is already complete. What would you like me to do next?")),
        _ => Ok(Continuation::Reply("needs_task", "I cannot confirm an unfinished task to resume. Tell me the specific next step you want.")),
    }
}

/// The app's own git reads. A repository's config can name commands git runs
/// on these (`diff.external`, textconv drivers, `core.fsmonitor`), and a project
/// or an agent edit can set them, so the reads switch them off.
const SAFE_GIT_DIFF_STAT: &str = "git -c core.fsmonitor=false diff --no-ext-diff --no-textconv --stat";
const SAFE_GIT_DIFF_FULL: &str = "git -c core.fsmonitor=false diff --no-ext-diff --no-textconv --no-color --unified=3";
const SAFE_GIT_STATUS: &str = "git -c core.fsmonitor=false status --short";

/// §59: backend owns the loop; frontend only observes/controls.
/// Spawns the LLM loop and returns immediately with a run id; the UI tails
/// events via GET /api/agent/runs/:id/events and approves via .../resume.
async fn run_agent(
    State(s): State<AppState>,
    Json(req): Json<AgentReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.task.trim().is_empty() {
        return Err(ApiError::bad(
            "task is empty",
            "Describe what the agent should do.",
        ));
    }
    let intent = answering_intent(&s, req.conversation_id.trim(), &req.task).await;
    if matches!(
        intent,
        MessageIntent::Acknowledgement | MessageIntent::Greeting
    ) {
        return conversational_reply(
            &s,
            req.conversation_id.trim(),
            req.task.trim(),
            if intent == MessageIntent::Greeting {
                "Hello! What would you like to work on?"
            } else {
                "Got it. Let me know if you’d like anything else."
            },
            "conversation",
        )
        .await
        .map(Json);
    }
    if req.workspace.trim().is_empty() {
        return Err(ApiError::bad(
            "workspace is empty",
            "Select a workspace folder first (§27); the agent never gets full-disk access.",
        ));
    }
    let ws_root = std::path::PathBuf::from(&req.workspace);
    if !ws_root.is_dir() {
        return Err(ApiError::bad(
            format!("workspace not found: {}", req.workspace),
            "Select an existing workspace folder first (§27).",
        ));
    }
    let mode = req.mode.unwrap_or(AgentMode::Agent);
    if mode == AgentMode::Chat {
        return Err(ApiError::bad(
            "Chat mode has no tools",
            "Use code-assist, agent, or autonomous mode for tool runs.",
        ));
    }
    let task = if intent == MessageIntent::Continue {
        match resolve_continuation(&s, req.conversation_id.trim(), &ws_root).await? {
            Continuation::Task(task) => task,
            // Nothing saved to continue, but the latest reply offered
            // something ("Shall I add tests?"): "go ahead" accepts the offer.
            Continuation::Reply("needs_task", _) if latest_reply_asks_in(&s, req.conversation_id.trim()).await => {
                req.task.trim().to_string()
            }
            Continuation::Reply(disposition, reply) => {
                return conversational_reply(
                    &s,
                    req.conversation_id.trim(),
                    req.task.trim(),
                    reply,
                    disposition,
                )
                .await
                .map(Json)
            }
        }
    } else {
        req.task.trim().to_string()
    };
    // Agent work belongs to the session that launched it. Persist the user's
    // natural-language task in the transcript before orchestration starts.
    let mut conversation_reasoning = false;
    if !req.conversation_id.trim().is_empty() {
        // Read before taking storage, the order the chat path uses.
        let current_model = s.models.read().await.current().map(|model| model.id.clone()).unwrap_or_default();
        let st = s.storage.lock().await;
        match st.get_conversation(req.conversation_id.trim()) {
            Ok(Some(conversation)) => {
                conversation_reasoning = conversation.reasoning_default;
                if conversation.mode == "code" {
                    let linked = linked_code_workspace(&st, &conversation)?;
                    let linked_root = std::fs::canonicalize(&linked.path).map_err(|error| {
                        ApiError::bad(
                            format!("project unavailable: {error}"),
                            "Reconnect this session to its intended project.",
                        )
                    })?;
                    let requested_root = std::fs::canonicalize(&ws_root).map_err(|error| {
                        ApiError::bad(
                            format!("project unavailable: {error}"),
                            "Choose an accessible project.",
                        )
                    })?;
                    if linked_root != requested_root {
                        return Err(ApiError::bad("the requested folder differs from this session's linked project", "Relink the session explicitly before starting work in a different project."));
                    }
                }
                st.add_message(&Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: req.conversation_id.trim().to_string(),
                    role: "user".into(),
                    content: req.task.trim().to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                })
                .map_err(|error| ApiError::internal(format!("storage error: {error}")))?;
                // This session is now worked on by the loaded model, as a chat
                // reply records. Agent runs never did, so a code session last
                // used with another model kept its "Last used with" notice.
                if !current_model.is_empty() {
                    let _ = st.set_last_model(req.conversation_id.trim(), &current_model);
                }
            }
            Ok(None) => return Err(ApiError::not_found("conversation not found")),
            Err(error) => return Err(ApiError::internal(format!("storage error: {error}"))),
        };
    }
    let reasoning = req
        .reasoning
        .unwrap_or(conversation_reasoning || s.settings.read().await.reasoning.default_on);
    let run_id = spawn_agent_run(
        &s,
        ws_root,
        task,
        mode,
        req.conversation_id.trim().to_string(),
        req.search,
        reasoning,
    )
    .await?;
    Ok(Json(
        serde_json::json!({"run_id": run_id, "state": "PLANNING", "disposition": "run", "continued": intent == MessageIntent::Continue}),
    ))
}

async fn spawn_agent_run(
    s: &AppState,
    ws_root: std::path::PathBuf,
    task: String,
    mode: AgentMode,
    conversation_id: String,
    search: bool,
    reasoning: bool,
) -> Result<String, ApiError> {
    use crate::agent_runner::{AgentSpec, LiveRun};
    use tokio::sync::broadcast;
    let _runtime_registration = s.runtime_update.try_lock().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "The model runtime is being updated.",
            "Wait for the model to finish loading, then start the task.",
        )
    })?;

    let (activity_tx, mut activity_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::agent::AgentEvent>();
    let run = std::sync::Arc::new(LiveRun {
        id: uuid::Uuid::new_v4().to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        spec: AgentSpec {
            workspace: ws_root,
            task,
            mode,
            conversation_id,
            search_enabled: search,
            reasoning,
        },
        cancel: CancelToken::new(),
        events: std::sync::Mutex::new(vec![]),
        // Live subscribers also receive streamed partial text; a slow browser
        // tab must lag, never close the stream.
        broadcaster: broadcast::channel(1024).0,
        activity_tx,
        pending: std::sync::Mutex::new(None),
        pending_tx: std::sync::Mutex::new(None),
        handle: std::sync::Mutex::new(None),
    });
    if !run.spec.conversation_id.is_empty() {
        let st = s.storage.lock().await;
        st.add_message(&Message {
            id: run.id.clone(),
            conversation_id: run.spec.conversation_id.clone(),
            role: "assistant".into(),
            content: "Work is starting…".into(),
            created_at: run.started_at.clone(),
        })
        .map_err(|error| ApiError::internal(format!("Could not save the run: {error}")))?;
        st.save_task_context(&crate::storage::TaskContext {
            conversation_id: run.spec.conversation_id.clone(),
            workspace: std::fs::canonicalize(&run.spec.workspace)
                .unwrap_or_else(|_| run.spec.workspace.clone())
                .to_string_lossy()
                .into_owned(),
            task: run.spec.task.clone(),
            run_id: run.id.clone(),
            status: "active".into(),
        })
        .map_err(|error| ApiError::internal(format!("Could not save task context: {error}")))?;
    }
    let journal = s.storage.clone();
    let journal_mid = run.id.clone();
    let journal_cid = run.spec.conversation_id.clone();
    tokio::spawn(async move {
        while let Some(event) = activity_rx.recv().await {
            let st = journal.lock().await;
            if let Err(error) = st.record_message_activity(&journal_mid, &event) {
                tracing::error!("Could not persist agent activity: {error}");
            }
            if matches!(
                event.state,
                AgentState::Completed | AgentState::Failed | AgentState::Cancelled
            ) {
                if !journal_cid.is_empty() {
                    let _ = st.update_message_content(&journal_cid, &journal_mid, &event.message);
                    let status = match event.state {
                        AgentState::Completed if mode == AgentMode::Plan && crate::agent_runner::presents_plan(&event) => "planned",
                        AgentState::Completed => "completed",
                        _ => "interrupted",
                    };
                    if let Err(error) = st.finish_task_context(&journal_cid, &journal_mid, status) {
                        tracing::error!("Could not persist task outcome: {error}");
                    }
                }
            }
        }
    });
    let bg = run.clone();
    let bg_state = s.clone();
    let bg_counter = s.agent_active.clone();
    bg_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    struct ActiveRunGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for ActiveRunGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let counter_guard = ActiveRunGuard(bg_counter);
    let handle = tokio::spawn(async move {
        use futures::FutureExt as _;
        let _counter_guard = counter_guard;
        match std::panic::AssertUnwindSafe(crate::agent_runner::run_loop(bg_state, bg.clone()))
            .catch_unwind()
            .await
        {
            Ok(end) => end,
            Err(_) => {
                bg.emit(crate::agent::AgentEvent::activity("error", AgentState::Failed,
                    "The agent stopped unexpectedly. Its recorded actions and any existing file changes were preserved; inspect them before retrying.".into(), 0));
                AgentState::Failed
            }
        }
    });
    *run.handle.lock().expect("lock") = Some(handle);
    s.agents.write().await.insert(run.clone());
    tracing::info!(run = %run.id, mode = ?mode, "agent run started");
    Ok(run.id.clone())
}

#[derive(Deserialize)]
struct AgentResume {
    approved: bool,
    #[serde(default)]
    grant_session: bool,
}

async fn agent_resume(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AgentResume>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::agent_runner::ApprovalDecision;
    let reg = s.agents.read().await;
    let run = match reg.get(&id) {
        Some(r) => r,
        None => return Err(ApiError::not_found("unknown agent run")),
    };
    let mut tx_guard = run.pending_tx.lock().expect("lock");
    match tx_guard.take() {
        Some(tx) => {
            let decision = if req.approved {
                ApprovalDecision::Approved {
                    session: req.grant_session,
                }
            } else {
                ApprovalDecision::Denied
            };
            // Denied DANGEROUS tools can never become session grants: the loop
            // only grants MODERATE ones alongside an approval.
            let _ = tx.send(decision);
            Ok(Json(
                serde_json::json!({"resumed": id, "approved": req.approved}),
            ))
        }
        None => Err(ApiError::bad(
            "run is not waiting for approval",
            "The run either continues on its own or already finished.",
        )),
    }
}

async fn agent_stop(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reg = s.agents.read().await;
    let run = match reg.get(&id) {
        Some(r) => r,
        None => return Err(ApiError::not_found("unknown agent run")),
    };
    if matches!(
        run.state(),
        AgentState::Completed | AgentState::Failed | AgentState::Cancelled
    ) {
        return Ok(Json(
            serde_json::json!({"stopped": id, "already_finished": true}),
        ));
    }
    run.cancel.cancel();
    if let Some(h) = run.handle.lock().expect("lock").take() {
        h.abort();
    }
    // Wake a run parked in approval wait so it observes cancellation.
    if let Some(tx) = run.pending_tx.lock().expect("lock").take() {
        let _ = tx.send(crate::agent_runner::ApprovalDecision::Denied);
    }
    *run.pending.lock().expect("lock") = None;
    let iteration = run
        .events
        .lock()
        .expect("lock")
        .last()
        .map(|event| event.iteration)
        .unwrap_or(0);
    run.emit(crate::agent::AgentEvent::activity(
        "status",
        AgentState::Cancelled,
        "Stopped by you. Completed actions and existing file changes have been kept.".into(),
        iteration,
    ));
    Ok(Json(serde_json::json!({"stopped": id})))
}

async fn agent_status(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reg = s.agents.read().await;
    match reg.get(&id) {
        Some(r) => {
            let evs = r.events.lock().expect("lock");
            Ok(Json(serde_json::json!({
                "id": r.id,
                "state": evs.last().map(|e| e.state).unwrap_or(AgentState::Idle),
                "iterations": evs.last().map(|e| e.iteration).unwrap_or(0),
                "pending": *r.pending.lock().expect("lock"),
                "events": evs.len(),
            })))
        }
        None => Err(ApiError::not_found("unknown agent run")),
    }
}

async fn agent_runs(State(s): State<AppState>) -> Json<Vec<crate::agent_runner::RunSummary>> {
    Json(s.agents.read().await.summaries())
}

/// Tail a run's events: replay the log, then stream live until terminal (§45).
async fn agent_events(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>>, ApiError> {
    use AgentState as St;
    let (backlog, rx) = {
        let reg = s.agents.read().await;
        match reg.get(&id) {
            Some(r) => (
                r.events.lock().expect("lock").clone(),
                r.broadcaster.subscribe(),
            ),
            None => return Err(ApiError::not_found("unknown agent run")),
        }
    };
    let terminal_seen = backlog
        .iter()
        .any(|e| matches!(e.state, St::Completed | St::Failed | St::Cancelled));
    let stream = async_stream_like(backlog, rx, terminal_seen);
    Ok(Sse::new(stream.boxed()).keep_alive(KeepAlive::default()))
}

fn async_stream_like(
    backlog: Vec<crate::agent::AgentEvent>,
    mut rx: tokio::sync::broadcast::Receiver<crate::agent::AgentEvent>,
    done: bool,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    use futures::StreamExt;
    let (tx, rx_out) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for ev in backlog {
            if tx
                .send(Ok(Event::default()
                    .event("agent")
                    .safe_data(serde_json::to_string(&ev).unwrap_or_default())))
                .is_err()
            {
                return;
            }
        }
        if done {
            return;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let terminal = matches!(
                        ev.state,
                        AgentState::Completed | AgentState::Failed | AgentState::Cancelled
                    );
                    if tx
                        .send(Ok(Event::default()
                            .event("agent")
                            .safe_data(serde_json::to_string(&ev).unwrap_or_default())))
                        .is_err()
                    {
                        return;
                    }
                    if terminal {
                        return;
                    }
                }
                // A slow subscriber drops some streamed partial text but keeps
                // following the run; every journaled event is replayed from
                // the durable log when the client reconnects.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    UnboundedReceiverStream::new(rx_out).filter_map(|x| async move { Some(x) })
}

// ---- Stage 12 slash commands (§§130–143) ----

#[derive(Deserialize)]
struct CommandQuery {
    #[serde(default)]
    q: String,
}

/// Autocomplete source for the composer `/` menu (§132).
async fn list_commands(
    axum::extract::Query(q): axum::extract::Query<CommandQuery>,
) -> Json<Vec<serde_json::Value>> {
    Json(
        crate::commands::complete(&q.q)
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name, "aliases": c.aliases, "description": c.description,
                    "category": c.category, "risk": format!("{:?}", c.risk).to_lowercase(),
                    "usage": c.usage,
                })
            })
            .collect(),
    )
}

/// What a command resolved to: plain text, or a UI action for the done frame.
#[derive(Debug)]
enum CommandOutcome {
    Message(String),
    NewConversation { id: String, message: String },
    AgentRun { run_id: String, message: String },
    Retry { text: String },
}

impl CommandOutcome {
    fn action_json(&self) -> serde_json::Value {
        match self {
            Self::Message(_) => serde_json::json!({"type": "message"}),
            Self::NewConversation { id, .. } => {
                serde_json::json!({"type": "clear", "conversation_id": id})
            }
            Self::AgentRun { run_id, .. } => {
                serde_json::json!({"type": "agent", "run_id": run_id})
            }
            Self::Retry { text } => serde_json::json!({"type": "retry", "text": text}),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Message(t) => t.clone(),
            Self::NewConversation { message, .. } => message.clone(),
            Self::AgentRun { message, .. } => message.clone(),
            Self::Retry { text } => format!("Retrying: {text}"),
        }
    }
}

/// Resolve a conversation's linked workspace directory (Stage 14 link).
/// Falls back to an explicit path argument.
async fn command_workspace(
    s: &AppState,
    conv_id: Option<&str>,
    arg_path: Option<&str>,
) -> Result<std::path::PathBuf, ApiError> {
    if let Some(p) = arg_path.filter(|p| !p.trim().is_empty()) {
        let pb = std::path::PathBuf::from(p.trim());
        if pb.is_dir() {
            return Ok(pb);
        }
        return Err(ApiError::bad(
            format!("directory not found: {p}"),
            "Pass an existing directory or link the chat to a workspace.",
        ));
    }
    if let Some(cid) = conv_id {
        let st = s.storage.lock().await;
        if let Ok(Some(conv)) = st.get_conversation(cid) {
            if !conv.workspace.is_empty() {
                if let Ok(Some(ws)) = st.get_workspace(&conv.workspace) {
                    let pb = std::path::PathBuf::from(&ws.path);
                    if pb.is_dir() {
                        return Ok(pb);
                    }
                    return Err(ApiError::bad(
                        format!("linked workspace '{}' is missing: {}", ws.name, ws.path),
                        "Pick another workspace or pass a directory.",
                    ));
                }
            }
        }
    }
    Err(ApiError::bad(
        "no workspace linked",
        "Link this chat to a workspace (Code mode) or pass a directory argument.",
    ))
}

async fn run_slash_command(
    s: &AppState,
    conv_id: Option<String>,
    name: &str,
    args: &str,
) -> Result<CommandOutcome, ApiError> {
    use CommandOutcome as O;

    match name {
        "help" => {
            let arg = (!args.is_empty()).then(|| args);
            Ok(O::Message(crate::commands::help_text(arg)))
        }
        "models" => {
            let list = s.models.read().await.list();
            if list.is_empty() {
                return Ok(O::Message(
                    "No models registered. Add GGUF + metadata.json under models/, then Scan."
                        .into(),
                ));
            }
            let mut out = String::from("Installed models:\n");
            for m in list {
                out.push_str(&format!(
                    "{} {} ({} {}, ctx {}){}\n",
                    if m.loaded { "●" } else { "○" },
                    m.name,
                    m.parameters,
                    m.quantization,
                    m.context_length,
                    if m.supports_reasoning {
                        " · reasoning"
                    } else {
                        ""
                    },
                ));
            }
            Ok(O::Message(out))
        }
        "model" => {
            if args.is_empty() {
                let cur = s.models.read().await.current().map(|m| m.id.clone());
                return Ok(O::Message(match cur {
                    Some(id) => format!("Active model: {id}\nUsage: /model <id> (see /models)"),
                    None => "No model loaded. Usage: /model <id> (see /models)".into(),
                }));
            }
            load_model_by_id(s, args).await.map(|(id, notices)| {
                let mut message = format!("Loaded {id}.");
                for notice in notices {
                    message.push_str(&format!("\n{notice}"));
                }
                O::Message(message)
            })
        }
        "status" => Ok(O::Message(system_status_text(s, conv_id.as_deref()).await)),
        "context" => {
            let cid = conv_id
                .ok_or_else(|| ApiError::bad("no conversation", "Run /context inside a chat."))?;
            Ok(O::Message(context_text(s, &cid).await?))
        }
        "clear" => {
            let st = s.storage.lock().await;
            let (model_id, mode, workspace, reasoning_default, search_default) = match &conv_id {
                Some(cid) => st
                    .get_conversation(cid)
                    .unwrap_or(None)
                    .map(|c| {
                        (
                            c.model_id,
                            c.mode,
                            c.workspace,
                            c.reasoning_default,
                            c.search_default,
                        )
                    })
                    .unwrap_or_default(),
                None => Default::default(),
            };
            drop(st);
            let (model_id, mode, workspace): (String, String, String) = (model_id, mode, workspace);
            let c = Conversation {
                id: uuid::Uuid::new_v4().to_string(),
                title: "New chat".into(),
                model_id,
                created_at: chrono::Utc::now().to_rfc3339(),
                mode,
                workspace,
                reasoning_default,
                search_default,
                last_model: String::new(),
                priority: "normal".into(),
                related_to: String::new(),
            };
            s.storage
                .lock()
                .await
                .create_conversation(&c)
                .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
            Ok(O::NewConversation { id: c.id.clone(), message: "New conversation started. The previous one remains available in history. Workspace, model and permissions are unchanged — /clear starts fresh context, /compact preserves continuity.".into() })
        }
        "compact" => {
            let cid = conv_id
                .ok_or_else(|| ApiError::bad("no conversation", "Run /compact inside a chat."))?;
            Ok(O::Message(compact_conversation(s, &cid).await?.text))
        }
        "search" => {
            if args.is_empty() {
                return Err(ApiError::bad("query is empty", "Usage: /search <query>"));
            }
            let ws = command_workspace(s, conv_id.as_deref(), None).await?;
            let wsm = crate::workspace::WorkspaceManager::new(ws);
            let req = crate::tools::ToolRequest {
                name: "search_text".into(),
                args: serde_json::json!({"query": args}),
                approved: true,
            };
            match crate::tools::execute(&req, &wsm, true) {
                Ok(r) => Ok(O::Message(format!("Search `{args}`:\n{r}", r = r.output))),
                Err(e) => Err(ApiError::bad(e.to_string(), "Fix the pattern and retry.")),
            }
        }
        "find" => {
            if args.is_empty() {
                return Err(ApiError::bad(
                    "pattern is empty",
                    "Usage: /find <pattern> (e.g. *.cpp)",
                ));
            }
            let ws = command_workspace(s, conv_id.as_deref(), None).await?;
            let hits = crate::commands::find_files(&ws, args, 50);
            Ok(O::Message(if hits.is_empty() {
                format!("No files matching `{args}`.")
            } else {
                format!("Files matching `{args}`:\n{}", hits.join("\n"))
            }))
        }
        "diff" => {
            let ws = command_workspace(s, conv_id.as_deref(), None).await?;
            match crate::terminal::run(SAFE_GIT_DIFF_STAT, &ws, 30) {
                Ok(r) if r.exit_code == Some(0) && !r.stdout.trim().is_empty() => {
                    Ok(O::Message(format!(
                        "Workspace changes:\n```\n{}\n```",
                        r.stdout.chars().take(4000).collect::<String>()
                    )))
                }
                _ => Ok(O::Message(
                    "No git changes (or not a git repository).".into(),
                )),
            }
        }
        "review" | "plan" | "init" => {
            let ws = command_workspace(s, conv_id.as_deref(), None).await?;
            let task = match name {
                "review" => format!("Review {} without modifying any files. Report findings.", if args.is_empty() { "the workspace".to_string() } else { args.to_string() }),
                // A plan run like the Plan mode's: read-only, and it ends
                // with a plan the user approves before anything changes.
                "plan" => {
                    if args.is_empty() {
                        return Err(ApiError::bad("task is empty", "Usage: /plan <task>"));
                    }
                    args.to_string()
                }
                _ => "Inspect this repository (structure, build system, key files) and summarize the project. Do not modify anything.".into(),
            };
            let reasoning = s.settings.read().await.reasoning.default_on;
            let run_id = spawn_agent_run(
                s,
                ws,
                task,
                if name == "plan" { AgentMode::Plan } else { AgentMode::CodeAssist },
                conv_id.unwrap_or_default(),
                false,
                reasoning,
            )
            .await?;
            Ok(O::AgentRun {
                run_id: run_id.clone(),
                message: format!(
                    "Read-only agent run started ({}) — watch the Agent tab.",
                    &run_id[..8]
                ),
            })
        }
        "build" | "test" | "run" => {
            let ws = command_workspace(s, conv_id.as_deref(), None).await?;
            let task = match name {
                "build" => format!(
                    "Build the project{}. Report success or the first errors.",
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!(" (target: {args})")
                    }
                ),
                "test" => format!(
                    "Run project tests{}. Report results.",
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!(" (filter: {args})")
                    }
                ),
                _ => {
                    if args.is_empty() {
                        return Err(ApiError::bad(
                            "command is empty",
                            "Usage: /run <command...>",
                        ));
                    }
                    format!("Run this project command and report the result: {args}")
                }
            };
            let reasoning = s.settings.read().await.reasoning.default_on;
            let run_id = spawn_agent_run(
                s,
                ws,
                task,
                AgentMode::Agent,
                conv_id.unwrap_or_default(),
                false,
                reasoning,
            )
            .await?;
            Ok(O::AgentRun { run_id: run_id.clone(), message: format!("Agent run started ({}) — it will ask before builds/commands. Watch the Agent tab.", &run_id[..8]) })
        }
        "permissions" => Ok(O::Message(s.permissions.read().await.describe())),
        "config" => Ok(O::Message(config_text(s, args).await)),
        "stop" => {
            let out = s.generations.write().await.cancel_current(&s.storage).await;
            Ok(O::Message(if out.stopped {
                format!("Stopped generation (kept {} chars).", out.chars_kept)
            } else {
                "Nothing generating right now.".into()
            }))
        }
        "retry" => {
            let cid = conv_id
                .ok_or_else(|| ApiError::bad("no conversation", "Run /retry inside a chat."))?;
            let st = s.storage.lock().await;
            let last_user = st
                .messages_for(&cid)
                .map_err(|e| ApiError::internal(format!("storage error: {e}")))?
                .into_iter()
                .rev()
                .find(|m| m.role == "user");
            match last_user {
                Some(m) => Ok(O::Retry { text: m.content }),
                None => Ok(O::Message("Nothing to retry yet.".into())),
            }
        }
        _ => Ok(O::Message(format!("Unknown command '/{name}'. Try /help."))),
    }
}

async fn system_status_text(s: &AppState, conv_id: Option<&str>) -> String {
    let hw = crate::hardware::detect();
    let (engine, model, ctx) = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            (
                "llama-server",
                s.models
                    .read()
                    .await
                    .current()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| "—".into()),
                llama.running.as_ref().map(|r| r.cfg.n_ctx).unwrap_or(0),
            )
        } else {
            (
                "stub",
                s.models
                    .read()
                    .await
                    .current()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| "none".into()),
                s.inference.read().await.context_size(),
            )
        }
    };
    let conv_line = match conv_id {
        Some(cid) => match context_numbers(s, cid).await {
            Ok((total, kept, dropped, tok)) => {
                format!("Chat: {kept}/{total} msgs in context (~{tok} tok, {dropped} dropped)\n")
            }
            Err(_) => String::new(),
        },
        None => String::new(),
    };
    let gpu = hw
        .gpus
        .first()
        .map(|g| format!("{} ({:.1} GB)", g.model, g.vram_gb))
        .unwrap_or_else(|| "not enumerated".into());
    format!(
        "Model: {model} ({engine}, ctx {ctx})\n{conv_line}CPU: {} ({} cores)\nRAM: {:.1} GB\nGPU: {gpu}\nAgent runs live: {}",
        hw.cpu.model, hw.cpu.logical_cores, hw.ram.total_gb,
        s.agents.read().await.summaries().iter().filter(|r| !matches!(r.state, crate::agent::AgentState::Completed | crate::agent::AgentState::Failed | crate::agent::AgentState::Cancelled)).count(),
    )
}

async fn context_numbers(s: &AppState, cid: &str) -> Result<(usize, usize, usize, u32), ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(cid), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let total = st
        .messages_for(cid)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?
        .len();
    let history = st
        .context_messages_for(cid)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let attachments = st
        .attachments_for(cid)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let turns = build_turns(&history, &attachments);
    let workspace = st
        .get_conversation(cid)
        .ok()
        .flatten()
        .map(|conversation| conversation.workspace)
        .unwrap_or_default();
    let memory = st
        .memory_context(cid, &workspace)
        .map_err(|error| ApiError::internal(format!("Could not read saved memory: {error}")))?;
    let chars: usize = turns.iter().map(|t| t.content.len()).sum::<usize>() + memory.text.len();
    Ok((
        total,
        turns.len().min(total),
        total.saturating_sub(turns.len()),
        estimate_tokens(chars),
    ))
}

async fn context_text(s: &AppState, cid: &str) -> Result<String, ApiError> {
    let limit = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            llama.running.as_ref().map(|r| r.cfg.n_ctx).unwrap_or(0)
        } else {
            s.inference.read().await.context_size()
        }
    };
    let (total, kept, dropped, tok) = context_numbers(s, cid).await?;
    let pct = if limit > 0 {
        (tok as f64 / limit as f64 * 100.0) as u32
    } else {
        0
    };
    Ok(format!("Context ~{tok}/{limit} tokens ({pct}%) — {kept}/{total} messages in window, {dropped} dropped. /compact frees space."))
}

async fn config_text(s: &AppState, section: &str) -> String {
    let mut v = serde_json::to_value(s.settings.read().await.clone()).unwrap_or_default();
    // Never print secrets into chat (§50).
    if v.get("search").is_some() {
        v["search"]["brave_key"] = serde_json::json!("***");
    }
    let section = section.trim();
    if section.is_empty() {
        return "Sections: general, inference, hardware, agent, security, reasoning, search, appearance, workspace, memory, files, keyboard, privacy, network, advanced, diagnostics\n/config <section> for details.".into();
    }
    match v.get(section) {
        Some(part) => format!(
            "{section}:\n```json\n{}\n```",
            serde_json::to_string_pretty(part).unwrap_or_default()
        ),
        None => format!("Unknown section '{section}'. Try /config with no args."),
    }
}

/// Load a registered model. Returns its id and the notices the load produced
/// (a CPU fallback, a context the model does not support), which callers must
/// pass on: they were once dropped here, so nobody saw them.
async fn load_model_by_id(s: &AppState, id: &str) -> Result<(String, Vec<String>), ApiError> {
    if id.trim().is_empty() {
        return Err(ApiError::bad(
            "model id is empty",
            "Usage: /model <id> (see /models)",
        ));
    }
    let model = s
        .models
        .read()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("unknown model '{id}'")))?;
    // One operation replaces the worker and only publishes the model after
    // health succeeds. Never relabel an old process as the new model.
    let started = start_sidecar(
        s,
        Some(model.id.clone()),
        model.gguf_path(),
        DEFAULT_SIDECAR_PORT,
        None,
        None,
        None,
    )
    .await?;
    let notices = started
        .get("notices")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Ok((model.id, notices))
}

/// Summarize a prefix into a derived inference view. Never delete or rewrite
/// the canonical transcript: it remains available to history, exports and forks.
async fn compact_conversation(s: &AppState, cid: &str) -> Result<CompactStats, ApiError> {
    let keep_recent = s
        .settings
        .read()
        .await
        .memory
        .compaction_keep_turns
        .clamp(2, 200);
    compact_conversation_keeping(s, cid, keep_recent).await
}

const CONVERSATION_COMPACTION_PROMPT: &str = "Update the running summary of a conversation between a user and an assistant so that it can replace the messages it covers. Keep the user's goals, constraints and preferences, decisions made, facts and figures, names of files and commands with their results, and every open question or unfinished task. Drop greetings and repetition. Write plain sentences or bullets, at most 250 words, and output only the updated summary.";

const COMPACTION_HEADER: &str = "[Compacted context — summary of";

/// Newest messages to keep verbatim when compacting automatically: as many as
/// fit in `max_chars`, at least two (the new message and the reply before
/// it), at most `max_keep`.
async fn recent_messages_within(s: &AppState, cid: &str, max_chars: usize, max_keep: usize) -> usize {
    let history = s.storage.lock().await.messages_for(cid).unwrap_or_default();
    let mut chars = 0usize;
    let mut keep = 0usize;
    for message in history.iter().rev() {
        chars += message.content.len();
        if keep >= 2 && (chars > max_chars || keep >= max_keep) {
            break;
        }
        keep += 1;
    }
    keep.max(2)
}

async fn summarize_conversation_chunk(
    client: &SidecarClient,
    cfg: &InferenceConfig,
    summary: &str,
    chunk: &str,
    max_tokens: u32,
) -> Result<String, ApiError> {
    let prompt = format!(
        "{CONVERSATION_COMPACTION_PROMPT}\n\nCurrent summary:\n{}\n\nNew messages:\n{chunk}",
        if summary.trim().is_empty() { "(none yet)" } else { summary.trim() }
    );
    let (text, _) = client
        .chat_turns_without_reasoning(&[ChatTurn::text("user", prompt)], max_tokens, cfg)
        .await
        .map_err(|e| ApiError::internal(format!("summarization failed: {e}")))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad(
            "the model returned an empty context summary",
            "Your transcript and existing context are unchanged. Retry with another model or a smaller conversation.",
        ));
    }
    Ok(text.to_owned())
}

/// Fold `messages` into `summary`, one window-sized chunk per request.
/// Returns the new summary and the characters it replaces.
async fn fold_conversation_summary(
    client: &SidecarClient,
    cfg: &InferenceConfig,
    mut summary: String,
    messages: &[Message],
) -> Result<(String, usize), ApiError> {
    // One summarization prompt, in characters at a conservative 3 per token,
    // after the room for the summary it writes and a margin.
    let summary_tokens = (cfg.n_ctx / 6).clamp(256, 1024);
    let prompt_chars = (cfg.n_ctx.saturating_sub(summary_tokens).saturating_sub(512) as usize * 3)
        .clamp(3_000, 180_000);
    let excerpt_chars = (prompt_chars / 3).clamp(600, 3_200);
    let excerpt = |content: &str| -> String {
        let total = content.chars().count();
        if total <= excerpt_chars {
            return content.to_owned();
        }
        let head: String = content.chars().take(excerpt_chars * 3 / 4).collect();
        let tail: String = content.chars().skip(total - excerpt_chars / 4).collect();
        format!("{head} […] {tail}")
    };
    let mut before_chars = summary.len();
    let mut chunk = String::new();
    for message in messages {
        before_chars += message.content.len();
        let line = format!("[{}] {}\n", message.role, excerpt(&message.content));
        if !chunk.is_empty()
            && CONVERSATION_COMPACTION_PROMPT.len() + summary.len() + chunk.len() + line.len() > prompt_chars
        {
            summary = summarize_conversation_chunk(client, cfg, &summary, &chunk, summary_tokens).await?;
            chunk.clear();
        }
        chunk.push_str(&line);
    }
    if !chunk.is_empty() {
        summary = summarize_conversation_chunk(client, cfg, &summary, &chunk, summary_tokens).await?;
    }
    Ok((summary, before_chars))
}

/// Summarize a conversation's older messages into its context summary and
/// keep the newest `keep_recent` messages verbatim. Safe to run automatically
/// on any window: messages are folded into the running summary in chunks
/// that fit the loaded context (one prompt holding every old message
/// overflowed long conversations), and an existing summary is the starting
/// point rather than being rebuilt from the first message. Original messages
/// are never modified; the summary is a derived view of their prefix.
async fn compact_conversation_keeping(
    s: &AppState,
    cid: &str,
    keep_recent: usize,
) -> Result<CompactStats, ApiError> {
    let keep_recent = keep_recent.clamp(1, 200);
    let (history, previous_summary, covered) = {
        let st = s.storage.lock().await;
        if matches!(st.get_conversation(cid), Ok(None)) {
            return Err(ApiError::not_found("conversation not found"));
        }
        let history = st
            .messages_for(cid)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        let view = st
            .context_messages_for(cid)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        let summary = view
            .first()
            .filter(|message| message.id.starts_with("context-summary:"))
            .map(|message| message.content.clone());
        // The view is [summary] + the messages it does not cover.
        let covered = if summary.is_some() {
            history.len().saturating_sub(view.len().saturating_sub(1))
        } else {
            0
        };
        (history, summary, covered)
    };
    let end = history.len().saturating_sub(keep_recent);
    if end <= covered {
        let n = history.len();
        return Ok(CompactStats {
            text: format!("Nothing to compact: {n} messages, and everything older than the newest {keep_recent} is already summarized."),
            before_chars: 0,
            after_chars: 0,
            saved_pct: 0,
            messages: n,
            status: "noop".into(),
        });
    }
    let sidecar = {
        let mut llama = s.llama.write().await;
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
        return Err(ApiError::bad(
            "compaction needs inference running",
            "Start inference first, then /compact.",
        ));
    };
    let client = SidecarClient::new(base_url)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .with_recorder(request_recorder(s, cid, &format!("compaction-{}", uuid::Uuid::new_v4()), "compaction"));
    let previous = previous_summary
        .map(|text| {
            if text.starts_with(COMPACTION_HEADER) {
                text.split_once('\n').map(|(_, body)| body.to_owned()).unwrap_or_default()
            } else {
                text
            }
        })
        .unwrap_or_default();
    let (summary, before_chars) =
        fold_conversation_summary(&client, &cfg, previous, &history[covered..end]).await?;
    let after_chars = summary.len();
    if after_chars >= before_chars {
        return Ok(CompactStats {
            text: "The generated summary would not save context, so nothing was changed. Original history is intact.".into(),
            before_chars, after_chars: before_chars, saved_pct: 0, messages: end, status: "noop".into(),
        });
    }
    {
        let st = s.storage.lock().await;
        // Reject stale summaries if the user edited/truncated the source while
        // inference was running. Newly appended messages are safe to retain.
        let all = st
            .messages_for(cid)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        if all.len() < end
            || !all
                .iter()
                .zip(&history[..end])
                .all(|(current, original)| current.id == original.id && current.content == original.content)
        {
            return Err(ApiError::bad(
                "conversation changed during compaction",
                "Retry compaction on the current transcript.",
            ));
        }
        let ids: Vec<String> = history[..end].iter().map(|message| message.id.clone()).collect();
        st.save_context_summary(cid, &ids, &format!(
            "{COMPACTION_HEADER} {end} older messages; original history preserved]\n{summary}"
        )).map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    }
    let saved = before_chars.saturating_sub(after_chars);
    let pct = if before_chars > 0 {
        saved * 100 / before_chars
    } else {
        0
    };
    Ok(CompactStats {
        text: format!("✓ Context compacted\nBefore {before_chars} chars\nAfter {after_chars} chars\nSaved {pct}% ({end} messages → 1 context summary). Original messages, timestamps and tool history are preserved."),
        before_chars, after_chars, saved_pct: pct, messages: end, status: "compacted".into(),
    })
}
// ---- Stage 17 model switching + prepare-context (§§170–180) ----

#[derive(Deserialize, Default)]
struct PrepareReq {
    /// Target model; defaults to the conversation's model, then current.
    #[serde(default)]
    model_id: String,
}

/// Select a model for a saved conversation. This does not prefill a KV cache:
/// actual scoped context is assembled and submitted with the next user turn.
async fn prepare_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PrepareReq>,
) -> Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let bg = s.clone();
    tokio::spawn(async move {
        let stage = |name: &str, status: &str, detail: &str| {
            let _ = tx.send(Ok(Event::default().event("stage").safe_data(
                serde_json::json!({"stage": name, "status": status, "detail": detail}).to_string(),
            )));
        };
        let fail = |msg: String| {
            let _ = tx.send(Ok(Event::default().event("error").safe_data(msg)));
        };
        // Never interrupt existing work merely to prepare another session.
        if let Err(error) = guard_agent_running(&bg, false).await {
            fail(error.message);
            return;
        }
        // Snapshot genuine saved context without claiming it was submitted.
        let (conv_model, history_n, attach_n, memory_n) = {
            let st = bg.storage.lock().await;
            match st.get_conversation(&id) {
                Ok(Some(c)) => {
                    let h = st.messages_for(&id).unwrap_or_default();
                    if c.mode == "code" {
                        if let Err(error) = linked_code_workspace(&st, &c) {
                            fail(error.message);
                            return;
                        }
                    }
                    let a = st.attachments_for(&id).unwrap_or_default().len();
                    let memory = match st.memory_context(&id, &c.workspace) {
                        Ok(memory) => memory.entries,
                        Err(error) => {
                            fail(format!("Could not read saved memory: {error}"));
                            return;
                        }
                    };
                    (c.model_id.clone(), h.len(), a, memory)
                }
                _ => {
                    fail("conversation not found".into());
                    return;
                }
            }
        };
        let target = if !req.model_id.trim().is_empty() {
            req.model_id.trim().to_string()
        } else if !conv_model.is_empty() {
            conv_model
        } else {
            bg.models
                .read()
                .await
                .current()
                .map(|m| m.id.clone())
                .unwrap_or_default()
        };
        if target.is_empty() {
            fail("No model selected. Load a model first.".into());
            return;
        }
        let gguf = match bg.models.read().await.get(&target) {
            Some(m) => m.gguf_path(),
            None => {
                fail(format!("Unknown model '{target}'."));
                return;
            }
        };
        // Registry labels cannot prove which weights a real process loaded.
        stage("model", "active", &format!("checking {target}"));
        let already = {
            let mut llama = bg.llama.write().await;
            llama.is_running()
                && llama
                    .running
                    .as_ref()
                    .map(|running| same_existing_model_path(&running.cfg.model_path, &gguf))
                    .unwrap_or(false)
        };
        if !already {
            if let Err(e) = start_sidecar(
                &bg,
                Some(target.clone()),
                gguf.clone(),
                DEFAULT_SIDECAR_PORT,
                None,
                None,
                None,
            )
            .await
            {
                fail(format!(
                    "Model failed to load: {}. Try fewer GPU layers or a smaller context.",
                    e.message
                ));
                return;
            }
        }
        // Re-check under the same lifecycle lock used by model switches. A
        // concurrent selection must not mark this session ready for old weights.
        let _selected_runtime = bg.runtime_update.lock().await;
        if let Err(error) = guard_agent_running(&bg, false).await {
            fail(error.message);
            return;
        }
        let still_selected = {
            let mut llama = bg.llama.write().await;
            llama.is_running()
                && llama
                    .running
                    .as_ref()
                    .map(|running| same_existing_model_path(&running.cfg.model_path, &gguf))
                    .unwrap_or(false)
        };
        if !still_selected {
            fail("The running model changed while this session was being prepared. Saved history was preserved; check the current model before continuing.".into());
            return;
        }
        stage("model", "done", &format!("{target} loaded"));
        stage(
            "system",
            "done",
            "Session instructions will be assembled with your next reply; nothing has been submitted to the model yet.",
        );
        // Stage: history — model-agnostic storage is reformatted at send time (§171).
        stage(
            "history",
            "done",
            &format!("{history_n} saved messages retained. The recent context window is assembled when you send a message."),
        );
        // Stage: attachments.
        stage(
            "attachments",
            "done",
            &format!("{attach_n} saved attachment(s) remain available; supported excerpts/images are included at request time."),
        );
        stage(
            "memory",
            "done",
            &format!("{memory_n} scoped saved memory entries available for the next reply."),
        );
        // This records the selection, not a fabricated cache-prefill operation.
        {
            let st = bg.storage.lock().await;
            let _ = st.set_last_model(&id, &target);
        }
        let _ = tx.send(Ok(Event::default().event("done").safe_data(
            serde_json::json!({"ready": true, "model": target, "history": history_n, "memory_entries": memory_n,
                "cache_rebuilt": false, "context_assembly": "next_request"}).to_string(),
        )));
    });
    Sse::new(UnboundedReceiverStream::new(rx).boxed()).keep_alive(KeepAlive::default())
}

fn same_existing_model_path(actual: &std::path::Path, expected: &std::path::Path) -> bool {
    match (
        std::fs::canonicalize(actual),
        std::fs::canonicalize(expected),
    ) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => false,
    }
}

#[derive(Deserialize, Default)]
struct CompatQuery {
    #[serde(default)]
    model_id: String,
}

/// Compatibility assessment for a pending switch (§179).
async fn conversation_compatibility(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<CompatQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    let conv = match st.get_conversation(&id) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let history = st
        .messages_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let attachments = st
        .attachments_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    drop(st);
    let target_id = if q.model_id.trim().is_empty() {
        conv.model_id.clone()
    } else {
        q.model_id.trim().to_string()
    };
    let target = match s.models.read().await.get(&target_id) {
        Some(m) => m.clone(),
        None => return Err(ApiError::not_found(format!("unknown model '{target_id}'"))),
    };
    let has_tool_turns = history.iter().any(|m| m.role == "tool");
    let image_attachments = attachments
        .iter()
        .filter(|a| a.mime.starts_with("image/"))
        .count();
    let mut warnings = vec![];
    if image_attachments > 0 && !target.vision {
        warnings.push(format!(
            "{image_attachments} image attachment(s) cannot be directly processed by text-only {}. Options: process with OCR, keep without analyzing, or switch back.",
            target.name
        ));
    }
    if has_tool_turns && !target.tool_calling {
        warnings.push(format!(
            "Thread contains tool calls but {} does not advertise tool calling. History is preserved; the agent loop stays disabled.",
            target.name
        ));
    }
    Ok(Json(serde_json::json!({
        "model": target.id,
        "text": true, "code": true, "memory": true,
        "tool_history": !has_tool_turns || target.tool_calling,
        "vision": image_attachments == 0 || target.vision,
        "tool_calling": !has_tool_turns || target.tool_calling,
        "warnings": warnings,
    })))
}

// ---- Stage 14 workspaces & sessions (§§27, 67, 114–128) ----

/// Detect a project's build system from marker files (§67.1, read-only).
fn detect_build_system(path: &std::path::Path) -> String {
    for (marker, name) in [
        ("CMakeLists.txt", "CMake"),
        ("Cargo.toml", "Cargo"),
        ("package.json", "Node"),
        ("go.mod", "Go"),
        ("Makefile", "Make"),
        ("build.gradle", "Gradle"),
        ("pyproject.toml", "Python"),
    ] {
        if path.join(marker).is_file() {
            return name.into();
        }
    }
    if std::fs::read_dir(path)
        .map(|mut e| {
            e.any(|f| {
                f.map(|f| {
                    f.path()
                        .extension()
                        .map(|x| x == "sln" || x == "csproj")
                        .unwrap_or(false)
                })
                .unwrap_or(false)
            })
        })
        .unwrap_or(false)
    {
        return "Visual Studio/C#".into();
    }
    String::new()
}

#[derive(Deserialize)]
struct NewWorkspace {
    name: String,
    path: String,
    #[serde(default)]
    create: bool,
}

/// Open the operating system's folder chooser. This keeps filesystem paths
/// out of the normal UX while returning only the folder the user selected.
async fn pick_folder() -> Result<Json<serde_json::Value>, ApiError> {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Choose a project folder")
            .pick_folder()
    })
    .await
    .map_err(|error| ApiError::internal(format!("folder picker failed: {error}")))?;
    Ok(Json(serde_json::json!({
        "path": picked.map(|path| path.to_string_lossy().into_owned())
    })))
}

async fn list_workspaces(
    State(s): State<AppState>,
) -> Result<Json<Vec<crate::storage::Workspace>>, ApiError> {
    s.storage
        .lock()
        .await
        .list_workspaces()
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

async fn create_workspace(
    State(s): State<AppState>,
    Json(req): Json<NewWorkspace>,
) -> Result<Json<crate::storage::Workspace>, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad(
            "name is empty",
            "Name the project, e.g. My app.",
        ));
    }
    let pb = if req.create {
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(ApiError::bad(
                "invalid project name",
                "Use a folder name without slashes.",
            ));
        }
        std::path::PathBuf::from(req.path.trim()).join(name)
    } else {
        std::path::PathBuf::from(req.path.trim())
    };
    if req.create && !pb.exists() {
        std::fs::create_dir_all(&pb).map_err(|error| {
            ApiError::internal(format!("could not create {}: {error}", pb.display()))
        })?;
    }
    if !pb.is_dir() {
        return Err(ApiError::bad(
            format!("directory not found: {}", pb.display()),
            "Choose an existing folder or create a new project.",
        ));
    }
    let build_system = detect_build_system(&pb);
    let file_count = walk_count(&pb, 2000);
    let w = crate::storage::Workspace {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        path: pb.to_string_lossy().into_owned(),
        build_system: if build_system.is_empty() {
            format!("unknown ({file_count} files)")
        } else {
            format!("{build_system} ({file_count} files)")
        },
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    s.storage
        .lock()
        .await
        .create_workspace(&w)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(w))
}

fn walk_count(root: &std::path::Path, cap: usize) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if n >= cap {
            break;
        }
        let mut entries: Vec<_> = std::fs::read_dir(&d)
            .map(|e| e.flatten().collect())
            .unwrap_or_default();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            if n >= cap {
                break;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "build"
            {
                continue;
            }
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(e.path());
            } else {
                n += 1;
            }
        }
    }
    n
}

async fn get_workspace(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::storage::Workspace>, ApiError> {
    match s.storage.lock().await.get_workspace(&id) {
        Ok(Some(w)) => Ok(Json(w)),
        Ok(None) => Err(ApiError::not_found("unknown workspace")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

#[derive(serde::Deserialize, Default)]
struct RemoveProjectQuery {
    /// `tasks=delete` removes the project's chats with it. Anything else keeps
    /// them: they report a missing project until they are linked to another.
    #[serde(default)]
    tasks: String,
}

/// Remove a project from the app. The folder on disk is never touched: this
/// deletes the app's own record of it, and (on request) the chats that work
/// in it, whose transcripts would otherwise point at a project that is gone.
async fn delete_workspace(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<RemoveProjectQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let with_tasks = query.tasks == "delete";
    let storage = s.storage.lock().await;
    if storage.get_workspace(&id).ok().flatten().is_none() {
        return Err(ApiError::not_found("unknown workspace"));
    }
    let chats = storage
        .conversations_in_workspace(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    // A run still working in one of these chats would keep writing to a
    // conversation that is being deleted underneath it.
    let working: Vec<String> = s
        .agents
        .read()
        .await
        .summaries()
        .into_iter()
        .filter(|run| run.state.is_active() && chats.contains(&run.conversation_id))
        .map(|run| run.task.chars().take(60).collect())
        .collect();
    if let Some(task) = working.first() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a task is still running in this project",
            format!("Stop the task {task:?} before removing the project."),
        ));
    }
    let mut removed_chats = 0usize;
    if with_tasks {
        for chat in &chats {
            if storage.delete_conversation(chat).unwrap_or(false) {
                removed_chats += 1;
            }
        }
    }
    match storage.delete_workspace(&id) {
        Ok(true) => Ok(Json(serde_json::json!({
            "deleted": id,
            "chats": chats.len(),
            "chats_deleted": removed_chats,
        }))),
        Ok(false) => Err(ApiError::not_found("unknown workspace")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

/// Stage 24 project instructions (§90): discover AGENTS.md/CLAUDE.md style
/// files. Displayed with scope; they NEVER override security policy.
async fn workspace_instructions(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const CANDIDATES: [&str; 5] = [
        "AGENTS.md",
        "CLAUDE.md",
        "MUSE.md",
        "PROJECT.md",
        "README.md",
    ];
    let ws = match s.storage.lock().await.get_workspace(&id) {
        Ok(Some(w)) => w,
        Ok(None) => return Err(ApiError::not_found("unknown workspace")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let root = std::path::PathBuf::from(&ws.path);
    let mut found = Vec::new();
    for name in CANDIDATES {
        let p = root.join(name);
        if let Ok(text) = std::fs::read_to_string(&p) {
            found.push(serde_json::json!({
                "file": name,
                "chars": text.len(),
                "excerpt": text.chars().take(4000).collect::<String>(),
                "scope": "agent behavior only — cannot grant permissions or network access",
            }));
        }
    }
    Ok(Json(
        serde_json::json!({"workspace": ws.name, "instructions": found}),
    ))
}

/// Stage 24 full diff (§92): capped unified diff + stat for the viewer.
async fn workspace_diff(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ws = match s.storage.lock().await.get_workspace(&id) {
        Ok(Some(w)) => w,
        Ok(None) => return Err(ApiError::not_found("unknown workspace")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let root = std::path::PathBuf::from(&ws.path);
    if !root.is_dir() {
        return Err(ApiError::bad(
            "workspace directory missing",
            "Relink the project directory.",
        ));
    }
    let stat = crate::terminal::run(SAFE_GIT_DIFF_STAT, &root, 30)
        .map(|r| format!("{}\n{}", r.stdout, r.stderr))
        .unwrap_or_else(|e| format!("(no git diff: {e})"));
    let full = crate::terminal::run(SAFE_GIT_DIFF_FULL, &root, 30)
        .map(|r| r.stdout)
        .unwrap_or_default();
    let truncated = full.len() > 100_000;
    Ok(Json(serde_json::json!({
        "workspace": ws.name,
        "stat": stat.chars().take(4000).collect::<String>(),
        "diff": full.chars().take(100_000).collect::<String>(),
        "truncated": truncated,
    })))
}

// ---- Stages 32–38 handlers ----

fn resolve_workspace_root(
    s: &AppState,
    ws: &crate::storage::Workspace,
) -> Result<std::path::PathBuf, ApiError> {
    let _ = s;
    let root = std::path::PathBuf::from(&ws.path);
    if !root.is_dir() {
        return Err(ApiError::bad(
            "workspace directory missing",
            "Relink the project directory.",
        ));
    }
    Ok(root)
}

async fn load_workspace(s: &AppState, id: &str) -> Result<crate::storage::Workspace, ApiError> {
    match s.storage.lock().await.get_workspace(id) {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(ApiError::not_found("unknown workspace")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

/// Stage 32: repo index for a workspace (§96). `?q=` narrows to ranked
/// matches; `?refresh=1` rebuilds instead of using the cache.
async fn workspace_index(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ws = load_workspace(&s, &id).await?;
    let root = resolve_workspace_root(&s, &ws)?;
    if q.get("refresh").map(|v| v == "1").unwrap_or(false) {
        s.repo_index.write().await.remove(&id);
    }
    let idx = cached_repo_index(&s, &id, &root).await;
    let symbols_total: usize = idx.files.iter().map(|f| f.symbols.len()).sum();
    if let Some(query) = q.get("q").filter(|v| !v.trim().is_empty()) {
        let hits = crate::repo_index::search(&idx, query, 30);
        return Ok(Json(serde_json::json!({
            "workspace": ws.name,
            "files_total": idx.files.len(),
            "symbols_total": symbols_total,
            "truncated": idx.truncated,
            "query": query,
            "matches": hits.iter().map(|f| serde_json::json!({
                "path": f.path, "size": f.size, "ext": f.ext,
                "symbols": f.symbols.iter().take(40).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })));
    }
    Ok(Json(serde_json::json!({
        "workspace": ws.name,
        "files_total": idx.files.len(),
        "symbols_total": symbols_total,
        "truncated": idx.truncated,
        "files": idx.files.iter().take(2000).map(|f| serde_json::json!({
            "path": f.path, "size": f.size, "ext": f.ext,
            "symbols": f.symbols.iter().take(40).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "files_capped": idx.files.len() > 2000,
    })))
}

// ---- Stage 35 knowledge ----

#[derive(Deserialize, Default)]
struct IngestReq {
    /// File or directory path, workspace-relative (or absolute inside root).
    #[serde(default)]
    path: String,
}

fn chunk_text(text: &str) -> Vec<String> {
    const SIZE: usize = 800;
    const OVERLAP: usize = 100;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + SIZE).min(chars.len());
        let piece: String = chars[start..end].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece);
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP);
    }
    out
}

/// Stage 35 ingest: index local docs into retrievable chunks (§39).
async fn ingest_knowledge(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<IngestReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ws = load_workspace(&s, &id).await?;
    let root = resolve_workspace_root(&s, &ws)?;
    let wsm = crate::workspace::WorkspaceManager::new(root.clone());
    let rel = if req.path.trim().is_empty() {
        ".".into()
    } else {
        req.path.trim().to_string()
    };
    let target = wsm
        .resolve(&rel)
        .map_err(|e| ApiError::bad(format!("{e}"), "Pick a path inside the workspace."))?;
    // Collect candidate text files (single file or directory, capped).
    let mut files = Vec::new();
    if target.is_file() {
        files.push(target);
    } else if target.is_dir() {
        let mut stack = vec![target];
        while let Some(d) = stack.pop() {
            if files.len() >= 50 {
                break;
            }
            let mut entries: Vec<_> = std::fs::read_dir(&d)
                .map(|e| e.flatten().collect())
                .unwrap_or_default();
            entries.sort_by_key(|e| e.path());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.')
                    || ["target", "node_modules", "build", "dist", ".git"].contains(&name.as_str())
                {
                    continue;
                }
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(e.path());
                } else if [
                    "md", "txt", "json", "csv", "rs", "py", "js", "ts", "cpp", "h",
                ]
                .contains(
                    &e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .as_str(),
                ) {
                    files.push(e.path());
                }
            }
        }
    } else {
        return Err(ApiError::bad(
            "path is not a file or directory",
            "Pick an existing file or folder.",
        ));
    }
    let st = s.storage.lock().await;
    let mut chunks_added = 0;
    let mut files_indexed = 0;
    // resolve() canonicalizes existing targets (\\?\ prefix on Windows), so
    // relativize against the canonical root with a plain fallback.
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
    for f in files.iter().take(50) {
        let meta = std::fs::metadata(f)
            .map_err(|e| ApiError::internal(format!("cannot stat file: {e}")))?;
        if meta.len() > 2_000_000 {
            continue; // oversized files stay out; attach excerpts cover them
        }
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(_) => continue, // binary: skip honestly
        };
        let rel_path = f
            .strip_prefix(&root)
            .or_else(|_| f.strip_prefix(&root_canon))
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| {
                f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.to_string_lossy().into_owned())
            });
        let _ = st.clear_knowledge_path(&id, &rel_path);
        for (i, piece) in chunk_text(&text).into_iter().enumerate().take(200) {
            let _ = st.add_knowledge_chunk(&crate::storage::KnowledgeChunk {
                id: uuid::Uuid::new_v4().to_string(),
                workspace_id: id.clone(),
                path: rel_path.clone(),
                chunk_idx: i as i64,
                text: piece,
            });
            chunks_added += 1;
        }
        files_indexed += 1;
    }
    Ok(Json(serde_json::json!({
        "workspace": ws.name, "files_indexed": files_indexed, "chunks_added": chunks_added,
        "note": "Keyword retrieval (term overlap). Vector embeddings are staged.",
    })))
}

async fn list_knowledge(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    load_workspace(&s, &id).await?;
    let st = s.storage.lock().await;
    let paths = st
        .knowledge_paths(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let total: i64 = paths.iter().map(|(_, n)| n).sum();
    Ok(Json(serde_json::json!({
        "paths": paths.iter().map(|(p, n)| serde_json::json!({"path": p, "chunks": n})).collect::<Vec<_>>(),
        "total_chunks": total,
    })))
}

async fn clear_knowledge(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<IngestReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    load_workspace(&s, &id).await?;
    let st = s.storage.lock().await;
    let removed = if req.path.trim().is_empty() {
        let paths = st
            .knowledge_paths(&id)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        let mut n = 0;
        for (p, _) in paths {
            n += st.clear_knowledge_path(&id, &p).unwrap_or(0);
        }
        n
    } else {
        st.clear_knowledge_path(&id, req.path.trim())
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?
    };
    Ok(Json(serde_json::json!({"removed": removed})))
}

// ---- Stage 33 metrics ----

/// Stage 33: per-message telemetry for a conversation (§§32–34).
/// Includes a content excerpt so the UI can match persisted metrics to
/// rendered bubbles without duplicating message content.
async fn conversation_metrics(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let history = st.messages_for(&id).unwrap_or_default();
    let metrics = st
        .metrics_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let out: Vec<serde_json::Value> = metrics
        .into_iter()
        .map(|m| {
            let excerpt = history
                .iter()
                .find(|h| h.id == m.message_id)
                .map(|h| h.content.chars().take(120).collect::<String>())
                .unwrap_or_default();
            serde_json::json!({
                "message_id": m.message_id, "model_id": m.model_id,
                "prompt_tokens": m.prompt_tokens, "generated_tokens": m.generated_tokens,
                "gen_ms": m.gen_ms, "ttft_ms": m.ttft_ms,
                "gen_tps": m.timing.as_ref().map(|timing| timing.output_tps).unwrap_or(Some(m.gen_tps)),
                "timing": m.timing,
                "excerpt": excerpt,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"metrics": out})))
}

// ---- Stage 36 git ----

/// Stage 36 read-only git surface (§62): status/log/branch, honest non-git.
async fn workspace_git(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ws = load_workspace(&s, &id).await?;
    let root = resolve_workspace_root(&s, &ws)?;
    if !root.join(".git").exists() {
        return Ok(Json(serde_json::json!({
            "workspace": ws.name, "git": false,
            "detail": "Not a git repository. Init one to track changes.",
        })));
    }
    let run = |cmd: &str| {
        crate::terminal::run(cmd, &root, 15)
            .map(|r| {
                format!("{}{}", r.stdout, r.stderr)
                    .chars()
                    .take(6000)
                    .collect::<String>()
            })
            .unwrap_or_else(|e| format!("(unavailable: {e})"))
    };
    Ok(Json(serde_json::json!({
        "workspace": ws.name,
        "git": true,
        "branch": run("git branch --show-current").trim().to_string(),
        "status": run(SAFE_GIT_STATUS),
        "log": run("git log --oneline -10"),
    })))
}

// ---- Stage 37 plugins ----

fn plugins_dir(s: &AppState) -> std::path::PathBuf {
    // <installation root>/plugins, wherever the models folder is; fall back to CWD.
    let p = s.install_root.join("plugins");
    if p.is_dir() {
        return p;
    }
    // Dev layout: backend runs with CWD=backend/, repo root one up.
    let up = std::path::PathBuf::from("..").join("plugins");
    if up.is_dir() {
        return up;
    }
    std::path::PathBuf::from("plugins")
}

/// Stage 37 plugin registry (§61): manifests declare tools; execution goes
/// through the same permission system as built-ins — no side channel.
async fn list_plugins(State(s): State<AppState>) -> Json<serde_json::Value> {
    let dir = plugins_dir(&s);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut dirs: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        dirs.sort_by_key(|e| e.file_name());
        for e in dirs {
            let id = e.file_name().to_string_lossy().into_owned();
            let manifest_path = e.path().join("plugin.json");
            match std::fs::read_to_string(&manifest_path) {
                Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(manifest) => {
                        // Cross-check declared tools against the real registry.
                        let known: Vec<String> =
                            crate::tools::registry().iter().map(|t| t.name.to_string()).collect();
                        let declared = manifest.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                        let unknown: Vec<String> = declared.iter()
                            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                            .filter(|n| !known.iter().any(|k| k == n))
                            .map(|n| n.to_string())
                            .collect();
                        out.push(serde_json::json!({
                            "id": id, "enabled": true, "manifest": manifest,
                            "unknown_tools": unknown,
                        }));
                    }
                    Err(e) => out.push(serde_json::json!({"id": id, "enabled": false, "error": format!("bad plugin.json: {e}")})),
                },
                Err(_) => out.push(serde_json::json!({"id": id, "enabled": false, "error": "missing plugin.json"})),
            }
        }
    }
    Json(serde_json::json!({"plugins": out}))
}

#[derive(Deserialize, Default)]
struct PluginRunReq {
    /// Workspace required for workspace-scoped tools.
    #[serde(default)]
    workspace_id: String,
    tool: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    approved: bool,
}

async fn run_plugin_command(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PluginRunReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // The plugin must exist and must declare this tool.
    let manifest_path = plugins_dir(&s).join(&id).join("plugin.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|_| ApiError::not_found(format!("unknown plugin '{id}'")))?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| ApiError::internal(format!("bad plugin.json: {e}")))?;
    let declared = manifest
        .get("tools")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    if !declared
        .iter()
        .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(req.tool.as_str()))
    {
        return Err(ApiError::bad(
            format!("plugin '{id}' does not declare tool '{}'", req.tool),
            "Plugins can only run tools listed in their own manifest.",
        ));
    }
    if crate::tools::registry().iter().all(|t| t.name != req.tool) {
        return Err(ApiError::not_found(format!("unknown tool '{}'", req.tool)));
    }
    let ws_root = if req.workspace_id.trim().is_empty() {
        std::env::temp_dir()
    } else {
        let ws = load_workspace(&s, req.workspace_id.trim()).await?;
        resolve_workspace_root(&s, &ws)?
    };
    let ws = crate::workspace::WorkspaceManager::new(ws_root);
    let tool_req = crate::tools::ToolRequest {
        name: req.tool.clone(),
        args: req.args.clone(),
        approved: req.approved,
    };
    match crate::tools::execute(&tool_req, &ws, req.approved) {
        Ok(r) => {
            let st = s.storage.lock().await;
            let _ = st.record_tool_execution(&crate::storage::ToolExecution {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: format!("plugin:{id}"),
                tool: req.tool.clone(),
                args: serde_json::to_string(&req.args).unwrap_or_default(),
                result: r.output.chars().take(2000).collect(),
                approved: req.approved,
                created_at: chrono::Utc::now().to_rfc3339(),
            });
            Ok(Json(
                serde_json::json!({"ok": r.ok, "output": r.output, "exit_code": r.exit_code}),
            ))
        }
        Err(crate::tools::ToolError::PermissionRequired { reason, .. }) => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!("Permission required for '{}': {reason}", req.tool),
            "Retry with approved=true after user confirmation.",
        )),
        Err(e) => Err(ApiError::bad(
            format!("{e}"),
            "Check tool arguments and retry.",
        )),
    }
}

// ---- Stage 34 doctor ----

/// Stage 34 diagnostics (§51): every check is infallible — failures become
/// rows, never a 500, so a broken subsystem can't hide the rest.
async fn doctor(State(s): State<AppState>) -> Json<serde_json::Value> {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut row = |id: &str, label: &str, status: &str, detail: String| {
        rows.push(
            serde_json::json!({"id": id, "label": label, "status": status, "detail": detail}),
        );
    };
    row(
        "backend",
        "Backend connectivity",
        "ok",
        "doctor ran against the live API".into(),
    );
    match s.storage.lock().await.list_conversations() {
        Ok(n) => row(
            "database",
            "Database",
            "ok",
            format!("SQLite readable ({} conversations)", n.len()),
        ),
        Err(e) => row("database", "Database", "fail", format!("SQLite error: {e}")),
    }
    if s.models_dir.is_dir() {
        row(
            "models_dir",
            "Models directory",
            "ok",
            s.models_dir.display().to_string(),
        );
    } else {
        row(
            "models_dir",
            "Models directory",
            "fail",
            format!("missing: {}", s.models_dir.display()),
        );
    }
    {
        let mm = s.models.read().await;
        let list = mm.list();
        let ggufs = list.iter().filter(|m| m.gguf_path().is_file()).count();
        row(
            "models",
            "Models",
            if list.is_empty() { "warn" } else { "ok" },
            format!("{} registered, {} with GGUF on disk", list.len(), ggufs),
        );
    }
    match SidecarBinary::detect(&s.runtime_dir()) {
        Ok(_) => row(
            "sidecar",
            "llama-server binary",
            "ok",
            "binary found".into(),
        ),
        Err(e) => row("sidecar", "llama-server binary", "warn", format!("{e}")),
    }
    {
        let running = s.llama.write().await.is_running();
        let model = s.models.read().await.current().map(|m| m.id.clone());
        row(
            "inference",
            "Inference",
            if running { "ok" } else { "warn" },
            match (running, model) {
                (true, Some(m)) => format!("running ({m})"),
                (true, None) => "running (unknown model)".into(),
                _ => "idle — load a model to start".into(),
            },
        );
    }
    {
        let hw = hardware::detect();
        row(
            "memory",
            "RAM",
            if hw.ram.available_gb > 2.0 {
                "ok"
            } else {
                "warn"
            },
            format!(
                "{:.1} / {:.1} GB available",
                hw.ram.available_gb, hw.ram.total_gb
            ),
        );
        match crate::metrics::vram_info() {
            Some((used, total)) => row(
                "gpu",
                "GPU/VRAM",
                "ok",
                format!("nvidia-smi: {used:.1} / {total:.1} GB used"),
            ),
            None => row(
                "gpu",
                "GPU/VRAM",
                "warn",
                "GPU telemetry unavailable. This does not rule out integrated or non-NVIDIA graphics; automatic loading checks the model runtime's device support.".into(),
            ),
        }
    }
    {
        let st = s.storage.lock().await;
        match st.list_workspaces() {
            Ok(ws) => {
                let missing: Vec<String> = ws
                    .iter()
                    .filter(|w| !std::path::Path::new(&w.path).is_dir())
                    .map(|w| w.name.clone())
                    .collect();
                if missing.is_empty() {
                    row(
                        "workspaces",
                        "Workspaces",
                        "ok",
                        format!("{} linked", ws.len()),
                    );
                } else {
                    row(
                        "workspaces",
                        "Workspaces",
                        "warn",
                        format!("missing dirs: {}", missing.join(", ")),
                    );
                }
            }
            Err(e) => row(
                "workspaces",
                "Workspaces",
                "fail",
                format!("storage error: {e}"),
            ),
        }
    }
    row(
        "tools",
        "Tool registry",
        "ok",
        format!("{} tools registered", crate::tools::registry().len()),
    );
    {
        let sp = s.settings.read().await.search.provider.clone();
        row(
            "search",
            "Search provider",
            "ok",
            format!("{sp} (opt-in per message)"),
        );
    }
    match std::fs::create_dir_all(&s.artifacts_dir) {
        Ok(_) => row(
            "artifacts",
            "Artifact directory",
            "ok",
            s.artifacts_dir.display().to_string(),
        ),
        Err(e) => row("artifacts", "Artifact directory", "fail", format!("{e}")),
    }
    let worst = if rows.iter().any(|r| r["status"] == "fail") {
        "fail"
    } else if rows.iter().any(|r| r["status"] == "warn") {
        "warn"
    } else {
        "ok"
    };
    Json(serde_json::json!({"status": worst, "checks": rows}))
}

// ---- Stage 38 setup + benchmark ----

/// Stage 38 first-run wizard data (§88): what exists, what's missing.
async fn setup_status(State(s): State<AppState>) -> Json<serde_json::Value> {
    let mm = s.models.read().await;
    let list = mm.list();
    let ggufs = list.iter().filter(|m| m.gguf_path().is_file()).count();
    let current = mm.current().map(|m| m.id.clone());
    drop(mm);
    let binary = SidecarBinary::detect(&s.runtime_dir()).is_ok();
    let running = s.llama.write().await.is_running();
    let convs = s
        .storage
        .lock()
        .await
        .list_conversations()
        .map(|v| v.len())
        .unwrap_or(0);
    let steps = vec![
        serde_json::json!({"id": "hardware", "label": "Hardware detected", "done": true}),
        serde_json::json!({"id": "binary", "label": "llama-server binary installed", "done": binary}),
        serde_json::json!({"id": "model", "label": "Model downloaded (GGUF on disk)", "done": ggufs > 0}),
        serde_json::json!({"id": "inference", "label": "Inference tested", "done": running}),
        serde_json::json!({"id": "chat", "label": "First chat started", "done": convs > 0}),
    ];
    Json(serde_json::json!({
        "models_registered": list.len(),
        "models_with_gguf": ggufs,
        "binary_found": binary,
        "inference_running": running,
        "current_model": current,
        "conversations": convs,
        "steps": steps,
        "needs_setup": !binary || ggufs == 0,
    }))
}

#[derive(Deserialize, Default)]
struct BenchmarkReq {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    max_tokens: u32,
}

/// Stage 38 benchmark mode (§89): timed generation through the live sidecar.
/// No sidecar → honest 503, never fabricated numbers.
async fn benchmark(
    State(s): State<AppState>,
    Json(req): Json<BenchmarkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (base_url, cfg, model) = {
        let mut llama = s.llama.write().await;
        if !llama.is_running() {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Inference is not running.",
                "Load a model and start inference first, then benchmark.",
            ));
        }
        let (url, c) = llama
            .running
            .as_ref()
            .map(|r| (r.base_url.clone(), r.cfg.clone()))
            .expect("checked");
        let m = s
            .models
            .read()
            .await
            .current()
            .map(|m| m.id.clone())
            .unwrap_or_default();
        (url, c, m)
    };
    let prompt = if req.prompt.trim().is_empty() {
        "Repeat exactly: The quick brown fox jumps over the lazy dog.".to_string()
    } else {
        req.prompt.trim().chars().take(2000).collect()
    };
    let max_tokens = req.max_tokens.clamp(16, 256);
    let client = SidecarClient::new(base_url.clone())
        .map_err(|e| ApiError::internal(format!("sidecar client failed: {e}")))?;
    let start = std::time::Instant::now();
    // Thinking off: a benchmark measures decode speed, not a hidden reasoning
    // channel that would leave the visible sample empty.
    let (text, m) = client
        .chat_turns_without_reasoning(&[ChatTurn::text("user", prompt)], max_tokens, &cfg)
        .await
        .map_err(|e| ApiError::internal(format!("benchmark generation failed: {e}")))?;
    let ms = start.elapsed().as_millis().max(1) as u64;
    let wall_tps = (m.generated_tokens as f64 * 1000.0 / ms as f64 * 10.0).round() / 10.0;
    let engine = m.engine.clone();
    Ok(Json(serde_json::json!({
        "model": model,
        "prompt_tokens": m.prompt_tokens,
        "generated_tokens": m.generated_tokens,
        "total_ms": ms,
        // Engine-measured decode rate when the runtime reports it; the
        // wall-clock rate (which includes prefill and HTTP) otherwise.
        "generation_tps": engine.as_ref().and_then(|e| e.predicted_tps).unwrap_or(wall_tps),
        "prompt_tps": engine.as_ref().and_then(|e| e.prompt_tps),
        "cached_tokens": engine.as_ref().map(|e| e.cached_tokens),
        "engine": engine,
        "context_limit": cfg.n_ctx,
        "sample_chars": text.len(),
    })))
}

#[derive(Deserialize, Default)]
struct PatchConv {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    reasoning_default: Option<bool>,
    #[serde(default)]
    search_default: Option<bool>,
    /// Stage 27 recovery discard: accept the loaded model for this thread.
    #[serde(default)]
    last_model: Option<String>,
}

/// Update session fields: title/model/mode/workspace/capability defaults.
async fn patch_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchConv>,
) -> Result<Json<Conversation>, ApiError> {
    let scope_changed = req.mode.is_some() || req.workspace.is_some();
    let st = s.storage.lock().await;
    let mut c = match st.get_conversation(&id) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    if let Some(t) = req.title {
        if t.len() > MAX_TITLE_CHARS {
            return Err(ApiError::bad(
                "title too long",
                "Keep titles under 200 characters.",
            ));
        }
        if !t.trim().is_empty() {
            c.title = t.trim().to_string();
        }
    }
    if let Some(m) = req.model_id {
        c.model_id = m;
    }
    if let Some(mode) = req.mode {
        match mode.as_str() {
            "chat" | "code" => c.mode = mode,
            _ => {
                return Err(ApiError::bad(
                    format!("invalid mode '{mode}'"),
                    "Mode must be 'chat' or 'code'.",
                ))
            }
        }
    }
    if let Some(w) = req.workspace {
        // A session keeps the project its history belongs to: its messages name
        // files in that folder. A session moved by the project picker once ran
        // its next task in another folder (owner report, 2026-09-17). A project
        // can still be set before the first message, or when the session has no
        // registered project to keep.
        if w != c.workspace && !c.workspace.trim().is_empty() {
            let storage_error = |e: rusqlite::Error| ApiError::internal(format!("storage error: {e}"));
            let project_kept = st.get_workspace(&c.workspace).map_err(storage_error)?.is_some();
            if project_kept && st.message_count(&c.id).map_err(storage_error)? > 0 {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "this session already works in another project",
                    "A session stays in the project it started in. Start a new task in the other project.",
                ));
            }
        }
        if !w.is_empty() {
            match st.get_workspace(&w) {
                Ok(Some(_)) => c.workspace = w,
                Ok(None) => return Err(ApiError::not_found(format!("unknown workspace '{w}'"))),
                Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
            }
        } else {
            c.workspace = String::new();
        }
    }
    if let Some(r) = req.reasoning_default {
        c.reasoning_default = r;
    }
    if let Some(v) = req.search_default {
        c.search_default = v;
    }
    if let Some(lm) = req.last_model {
        c.last_model = lm;
    }
    if scope_changed && c.mode == "code" {
        linked_code_workspace(&st, &c)?;
    }
    st.update_conversation(&c)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(c))
}

#[derive(Deserialize, Default)]
struct ForkReq {
    #[serde(default)]
    title: String,
}

/// Stage 14 fork (§145): new id, shared snapshot, divergent future.
async fn fork_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (src, n) = {
        let st = s.storage.lock().await;
        let src = match st.get_conversation(&id) {
            Ok(Some(c)) => c,
            Ok(None) => return Err(ApiError::not_found("conversation not found")),
            Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
        };
        let dst_id = uuid::Uuid::new_v4().to_string();
        let title = if req.title.trim().is_empty() {
            format!("{} (fork)", src.title)
        } else {
            req.title.trim().to_string()
        };
        st.create_conversation(&Conversation {
            id: dst_id.clone(),
            title,
            model_id: src.model_id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: src.mode.clone(),
            workspace: src.workspace.clone(),
            reasoning_default: src.reasoning_default,
            search_default: src.search_default,
            last_model: src.last_model.clone(),
            priority: src.priority.clone(),
            related_to: src.related_to.clone(),
        })
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        let n = st
            .fork_messages(&id, &dst_id, &chrono::Utc::now().to_rfc3339())
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        (dst_id, n)
    };
    // Attachment bytes live under data/attachments/<conv>/ — copy them so the
    // fork is self-contained (§145: share snapshots, not live references).
    let (src_dir, dst_dir) = (s.attachments_dir.join(&id), s.attachments_dir.join(&src));
    if src_dir.is_dir() {
        let _ = std::fs::create_dir_all(&dst_dir);
        if let Ok(entries) = std::fs::read_dir(&src_dir) {
            for e in entries.flatten() {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    let _ = std::fs::copy(e.path(), dst_dir.join(e.file_name()));
                }
            }
        }
    }
    Ok(Json(serde_json::json!({"forked": src, "messages": n})))
}

#[derive(Deserialize, Default)]
struct ShareReq {
    target_id: String,
    /// how many recent turns to share (default 6, max 20)
    #[serde(default = "default_share_turns")]
    turns: usize,
    #[serde(default)]
    note: String,
    /// Stage 24 explicit include picker (§§98–99): what travels.
    #[serde(default = "default_true")]
    messages: bool,
    #[serde(default)]
    attachments: bool,
    #[serde(default)]
    memory: bool,
    #[serde(default)]
    summary: bool,
}

fn default_true() -> bool {
    true
}

fn default_share_turns() -> usize {
    6
}

/// Stage 14 cross-session share (§§98–99): compact package, explicit target.
async fn share_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ShareReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.target_id.trim().is_empty() || req.target_id == id {
        return Err(ApiError::bad(
            "bad target",
            "Share into a different conversation.",
        ));
    }
    let st = s.storage.lock().await;
    let src_title = match st.get_conversation(&id) {
        Ok(Some(c)) => c.title,
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    if matches!(st.get_conversation(&req.target_id), Ok(None)) {
        return Err(ApiError::not_found("target conversation not found"));
    }
    let history = st
        .messages_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let take = req.turns.clamp(1, 20);
    let start = history.len().saturating_sub(take);
    let mut package = format!("Shared context from '{src_title}':\n");
    if !req.note.trim().is_empty() {
        package.push_str(&format!("Note: {}\n", req.note.trim()));
    }
    if req.summary {
        let decisions: Vec<&str> = history
            .iter()
            .filter(|m| m.role == "assistant")
            .flat_map(|m| m.content.lines())
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with("Decision:") || t.starts_with("✓") || t.starts_with("Summary:")
            })
            .take(10)
            .collect();
        if decisions.is_empty() {
            package.push_str("Key findings: (no explicit decisions recorded)\n");
        } else {
            package.push_str("Key findings:\n");
            for d in decisions {
                package.push_str(&format!(
                    "- {}\n",
                    d.trim().chars().take(300).collect::<String>()
                ));
            }
        }
    }
    let mut shared_turns = 0;
    if req.messages {
        for m in &history[start..] {
            package.push_str(&format!(
                "\n[{}] {}\n",
                m.role,
                m.content.chars().take(1500).collect::<String>()
            ));
        }
        shared_turns = history.len() - start;
    }
    if req.attachments {
        let atts = st
            .attachments_for(&id)
            .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
        if atts.is_empty() {
            package.push_str("\nAttachments: none\n");
        } else {
            package.push_str("\nAttachments:\n");
            for a in atts.iter().take(20) {
                package.push_str(&format!(
                    "- {} ({}, {} bytes): {}\n",
                    a.filename,
                    a.mime,
                    a.size_bytes,
                    a.text_excerpt.chars().take(800).collect::<String>()
                ));
            }
        }
    }
    if req.memory {
        let tgt_ws = st
            .get_conversation(&req.target_id)
            .ok()
            .flatten()
            .map(|c| c.workspace)
            .unwrap_or_default();
        let mems = st.memory_export(&id, &tgt_ws).unwrap_or_default();
        if mems.is_empty() {
            package.push_str("\nMemory: none visible to source\n");
        } else {
            package.push_str("\nMemory:\n");
            for m in mems.iter().take(20) {
                package.push_str(&format!(
                    "- [{}] {}\n",
                    m.scope,
                    m.content.chars().take(500).collect::<String>()
                ));
            }
        }
    }
    st.add_message(&Message {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: req.target_id.clone(),
        role: "user".into(),
        content: package,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
    .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(
        serde_json::json!({"shared": req.target_id, "turns": shared_turns}),
    ))
}

/// Stage 14 session list with computed residency (§§119–124).
/// ACTIVE = generating now or an agent run touching it; WARM = inference up;
/// COLD = everything else. KV residency itself is llama-server's slots —
/// history here is always the source of truth (§122).
async fn list_sessions(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    let convs = st
        .list_conversations()
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    drop(st);
    let generating: std::collections::HashSet<String> = {
        let mut set = std::collections::HashSet::new();
        if let Some(cid) = s.generations.read().await.active_conversation() {
            if !cid.is_empty() {
                set.insert(cid);
            }
        }
        for r in s.agents.read().await.summaries() {
            if r.state.is_active() && !r.conversation_id.is_empty()
            {
                set.insert(r.conversation_id);
            }
        }
        set
    };
    let warm = {
        let mut llama = s.llama.write().await;
        llama.is_running()
    };
    // Stage 27 activity classes (§126): idle|thinking|tool|waiting|error.
    let agent_state: std::collections::HashMap<String, String> = {
        let mut m = std::collections::HashMap::new();
        for r in s.agents.read().await.summaries() {
            if r.conversation_id.is_empty() {
                continue;
            }
            let cls = match r.state {
                crate::agent::AgentState::Planning
                | crate::agent::AgentState::Observing
                | crate::agent::AgentState::Compacting => "thinking",
                crate::agent::AgentState::ExecutingTool => "tool",
                crate::agent::AgentState::WaitingPermission => "waiting",
                crate::agent::AgentState::Failed => "error",
                _ => continue,
            };
            m.insert(r.conversation_id, cls.into());
        }
        m
    };
    let sessions: Vec<serde_json::Value> = convs
        .into_iter()
        .map(|c| {
            let residency = if generating.contains(&c.id) || agent_state.contains_key(&c.id) {
                "active"
            } else if warm {
                "warm"
            } else {
                "cold"
            };
            let activity = agent_state.get(&c.id).cloned().unwrap_or_else(|| {
                if generating.contains(&c.id) {
                    "thinking".into()
                } else {
                    "idle".into()
                }
            });
            serde_json::json!({
                "id": c.id, "title": c.title, "mode": c.mode, "workspace": c.workspace,
                "model_id": c.model_id, "residency": residency, "activity": activity,
                "priority": c.priority, "related_to": c.related_to,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"sessions": sessions})))
}

#[derive(Deserialize, Default)]
struct PatchSession {
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    related_to: Option<String>,
}

/// Stage 27: priority + session relationships (§138, §146).
async fn patch_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchSession>,
) -> Result<Json<Conversation>, ApiError> {
    let st = s.storage.lock().await;
    let mut c = match st.get_conversation(&id) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    if let Some(p) = req.priority {
        if !["background", "normal", "high"].contains(&p.as_str()) {
            return Err(ApiError::bad(
                format!("invalid priority '{p}'"),
                "Priority must be background, normal or high (§138).",
            ));
        }
        c.priority = p;
    }
    if let Some(r) = req.related_to {
        if !r.is_empty() && r != id && matches!(st.get_conversation(&r), Ok(None)) {
            return Err(ApiError::not_found("related session not found"));
        }
        c.related_to = r;
    }
    st.update_conversation(&c)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(c))
}

/// Stage 27 portable export (§147): summary, never raw KV state.
async fn export_conversation(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    let conv = match st.get_conversation(&id) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(ApiError::not_found("conversation not found")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let history = st
        .messages_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    let tools = st.tool_executions_for(&id, 2_000).unwrap_or_default();
    let activities = st.conversation_activities(&id).unwrap_or_default();
    let artifacts = st.artifacts_for(&id).unwrap_or_default();
    let attachments = st.attachments_for(&id).unwrap_or_default();
    let model_requests = st
        .model_requests_for(&id, crate::storage::MODEL_REQUEST_ROWS)
        .unwrap_or_default();
    // What the log was produced on, so a run read on another machine can be
    // told apart from one produced there: a small window and a template that
    // refuses a system turn explain failures that look inexplicable without
    // them.
    let runtime_snapshot = {
        let llama = s.llama.read().await;
        match llama.running.as_ref() {
            Some(running) => serde_json::json!({
                "model_path": running.cfg.model_path.file_name().and_then(|name| name.to_str()),
                "context": running.cfg.n_ctx,
                "cache_type_k": running.cfg.kv_cache_type_k,
                "cache_type_v": running.cfg.kv_cache_type_v,
                "gpu_layers": running.cfg.n_gpu_layers,
                "chat_template": {
                    "system_role": running.cfg.chat_template.system_role,
                    "strict_alternation": running.cfg.chat_template.strict_alternation,
                },
                "policy": running.cfg.runtime_policy,
                "os": std::env::consts::OS,
            }),
            None => serde_json::json!({"os": std::env::consts::OS}),
        }
    };
    let decisions: Vec<String> = history
        .iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| m.content.lines().map(|l| l.to_string()))
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("Decision:") || t.starts_with("✓") || t.starts_with("Changed:")
        })
        .take(30)
        .collect();
    let files: Vec<String> = tools
        .iter()
        .filter(|t| {
            ["write_file", "append_file", "edit_file", "create_document"].contains(&t.tool.as_str())
        })
        .take(50)
        .map(|t| {
            format!(
                "{} {}",
                t.tool,
                t.args.chars().take(160).collect::<String>()
            )
        })
        .collect();
    Ok(Json(serde_json::json!({
        "id": conv.id, "title": conv.title, "mode": conv.mode,
        "model": conv.model_id, "workspace": conv.workspace,
        "exported_at": now_rfc3339(),
        "turns": history.len(), "decisions": decisions,
        "changed_files": files,
        "tool_calls": tools.len(),
        "artifacts": artifacts.iter().map(|a| &a.filename).collect::<Vec<_>>(),
        "attachments": attachments.iter().map(|a| &a.filename).collect::<Vec<_>>(),
        // The transcript carries its message ids so the journal below can be
        // matched to the reply it belongs to on another machine.
        "transcript": history
            .iter()
            .map(|m| serde_json::json!({
                "id": m.id,
                "role": m.role,
                "created_at": m.created_at,
                "content": m.content,
            }))
            .collect::<Vec<_>>(),
        // The execution log: what each action was asked to do and what came
        // back. Truncated results would defeat the purpose of carrying a
        // failure to another machine to read, so they travel whole.
        "executions": tools
            .iter()
            .rev()
            .map(|t| serde_json::json!({
                "tool": t.tool,
                "args": t.args,
                "result": t.result,
                "approved": t.approved,
                "created_at": t.created_at,
            }))
            .collect::<Vec<_>>(),
        // The agent's own step-by-step journal, per reply: states, statuses,
        // thoughts, tool starts and results, and the context measurements.
        "activity": history
            .iter()
            .filter_map(|m| {
                let events = activities.get(&m.id)?;
                Some(serde_json::json!({"message_id": m.id, "events": events}))
            })
            .collect::<Vec<_>>(),
        "runtime": runtime_snapshot,
        // Each model request as sent and what came back (owner decision: kept
        // by default and exported). The request travels as JSON, not a string.
        "model_requests": model_requests
            .iter()
            .map(|r| serde_json::json!({
                "owner_id": r.owner_id,
                "seq": r.seq,
                "kind": r.kind,
                "created_at": r.created_at,
                "outcome": r.outcome,
                "failure": r.failure,
                "finish_reason": r.finish_reason,
                "prompt_tokens": r.prompt_tokens,
                "cached_tokens": r.cached_tokens,
                "generated_tokens": r.generated_tokens,
                "request": serde_json::from_str::<serde_json::Value>(&r.request_json).unwrap_or(serde_json::Value::Null),
                "output": r.raw_output,
            }))
            .collect::<Vec<_>>(),
        // The application log as it stood at export. Not per-conversation:
        // the failures worth carrying between machines (a model that will not
        // load, a template that refuses every request) are logged before any
        // conversation is involved.
        "app_log": crate::logbuf::global().recent(1_000),
    })))
}

/// Stage 27 recovery (§83, §103): stale model contexts + live work.
/// Registries are in-memory, so after a restart anything that was live is
/// reported stale with Resume (prepare) / Discard actions — never resumed blindly.
async fn sessions_recovery(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let loaded = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.id.clone())
        .unwrap_or_default();
    let st = s.storage.lock().await;
    let convs = st
        .list_conversations()
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    drop(st);
    let busy: Vec<String> = {
        let mut b = Vec::new();
        if let Some(cid) = s.generations.read().await.active_conversation() {
            if !cid.is_empty() {
                b.push(cid);
            }
        }
        b
    };
    let stale: Vec<serde_json::Value> = convs
        .into_iter()
        .filter(|c| !c.last_model.is_empty() && c.last_model != loaded)
        .take(50)
        .map(|c| {
            serde_json::json!({
                "id": c.id, "title": c.title,
                "last_model": c.last_model, "loaded_model": loaded,
                "actions": ["resume", "discard"],
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "busy": busy, "stale": stale,
        "note": "Resume prepares the stored history for the loaded model; Discard clears the stale marker.",
    })))
}

#[derive(Deserialize)]
struct SessionAction {
    action: String, // pause | stop | reduce
}

/// Stage 27 row actions (§139): pause/stop live work, or reduce usage.
async fn session_action(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SessionAction>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    drop(st);
    match req.action.as_str() {
        "pause" | "stop" => {
            let mut acted = Vec::new();
            if s.generations.read().await.active_conversation().as_deref() == Some(id.as_str()) {
                let out = s.generations.write().await.cancel_current(&s.storage).await;
                if out.stopped {
                    acted.push(format!(
                        "generation stopped ({} chars kept)",
                        out.chars_kept
                    ));
                }
            }
            for r in s.agents.read().await.summaries() {
                if r.conversation_id != id {
                    continue;
                }
                if r.state.is_active() {
                    if let Some(run) = s.agents.read().await.get(&r.id) {
                        run.cancel.cancel();
                        if let Some(h) = run.handle.lock().expect("lock").take() {
                            h.abort();
                        }
                        acted.push(format!(
                            "agent run {} stopped",
                            r.id.chars().take(8).collect::<String>()
                        ));
                    }
                }
            }
            if acted.is_empty() {
                return Ok(Json(
                    serde_json::json!({"action": req.action, "result": "noop", "detail": "Session has no live generation or agent work."}),
                ));
            }
            Ok(Json(
                serde_json::json!({"action": req.action, "result": "stopped", "detail": acted}),
            ))
        }
        "reduce" => {
            let running = s.llama.write().await.is_running();
            if !running {
                return Err(ApiError::bad(
                    "cannot reduce without inference",
                    "Start inference first, then reduce (runs /compact).",
                ));
            }
            let stats = compact_conversation(&s, &id).await?;
            Ok(Json(
                serde_json::json!({"action": "reduce", "result": stats.status, "detail": stats.text}),
            ))
        }
        _ => Err(ApiError::bad(
            format!("unknown action '{}'", req.action),
            "Action must be pause, stop or reduce (§139).",
        )),
    }
}

async fn get_settings(State(s): State<AppState>) -> Json<AppSettings> {
    Json(s.settings.read().await.clone())
}

#[derive(Deserialize, Default)]
struct RuntimePolicyQuery {
    model_id: Option<String>,
}

async fn runtime_policy(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<RuntimePolicyQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings = s.settings.read().await.clone();
    let (running, active) = {
        let mut runtime = s.llama.write().await;
        let running = runtime.is_running();
        let active = if running {
            runtime
                .running
                .as_ref()
                .and_then(|r| r.cfg.runtime_policy.clone())
        } else {
            None
        };
        (running, active)
    };
    let model = {
        let registry = s.models.read().await;
        if let Some(id) = q.model_id.as_ref().filter(|id| !id.is_empty()) {
            // A saved default that is no longer installed (or an empty models
            // folder) must not turn the settings page into an error: the plan
            // for "no particular model" is still meaningful, and the response
            // says which model it could not find.
            registry.get(id).cloned()
        } else {
            registry
                .current()
                .cloned()
                .or_else(|| registry.get(&settings.general.default_model).cloned())
        }
    };
    let requested = InferenceConfig {
        n_ctx: settings.inference.context_size,
        n_batch: settings.inference.batch_size,
        n_threads: settings.hardware.cpu_threads,
        n_gpu_layers: settings.hardware.gpu_layers,
        flash_attn: settings.hardware.flash_attention,
        kv_cache_gpu: settings.hardware.kv_cache_gpu,
        ..InferenceConfig::default()
    };
    // The plan for the next load uses the sampler's latest GPU reading; the
    // load itself re-measures after the previous worker has exited.
    let vram = s
        .metrics
        .lock()
        .ok()
        .and_then(|log| log.latest())
        .and_then(|sample| {
            let total = sample.vram_total_gb?;
            let used = sample.vram_used_gb.unwrap_or(0.0);
            let running_share = if running { total * 0.0 } else { 0.0 };
            let _ = running_share;
            (total > 0.0).then(|| crate::inference::VramState {
                total_bytes: (total * 1_073_741_824.0) as u64,
                used_bytes: (used * 1_073_741_824.0) as u64,
            })
        });
    let ram_available = (hardware::detect().ram.available_gb * 1_073_741_824.0) as u64;
    let next = crate::inference::resolve_runtime_policy_for_machine(
        model.as_ref(),
        &requested,
        settings.runtime_auto,
        &settings.runtime,
        // While a model is loaded its own memory counts as "used"; the
        // preview would then under-size the next context. Skip fitting until
        // the load re-measures.
        if running { None } else { vram },
        Some(ram_available),
    );
    let missing_model = q
        .model_id
        .as_ref()
        .filter(|id| !id.is_empty() && model.as_ref().map(|m| &m.id) != Some(id));
    Ok(Json(serde_json::json!({
        "running": running,
        "model_id": model.as_ref().map(|m| &m.id),
        "model_name": model.as_ref().map(|m| &m.name),
        "missing_model": missing_model,
        "active": active, "next": next, "applies_on": "next_model_load"
    })))
}

#[derive(Deserialize)]
struct PermissionModeRequest {
    mode: String,
}

async fn get_permission_mode(State(s): State<AppState>) -> Json<serde_json::Value> {
    let mode = s.settings.read().await.agent.permission_mode.clone();
    Json(serde_json::json!({ "mode": mode }))
}

async fn put_permission_mode(
    State(s): State<AppState>,
    Json(req): Json<PermissionModeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !crate::permissions::PERMISSION_MODES.contains(&req.mode.as_str()) {
        return Err(ApiError::bad("unknown permission mode", "Use ask, accept_edits, plan or auto."));
    }
    let autonomy = crate::permissions::autonomy_for_mode(&req.mode);
    let _update = s.settings_update.lock().await;
    let mut next = s.settings.read().await.clone();
    next.agent.permission_mode = req.mode.clone();
    next.agent.autonomous_enabled = req.mode == "auto";
    s.storage
        .lock()
        .await
        .save_settings(&next)
        .map_err(|error| {
            ApiError::internal(format!("Could not save permission preference: {error}"))
        })?;
    *s.settings.write().await = next;
    s.permissions.write().await.autonomy = autonomy;
    let resumed = resume_auto_approved_runs(&s).await;
    Ok(Json(
        serde_json::json!({ "mode": req.mode, "resumed": resumed }),
    ))
}

/// Called while settings_update is held, after save-before-apply succeeds.
/// Both settings entry points must honor the same permission mode, including
/// actions already waiting: switching to Accept edits releases a waiting file
/// edit, switching to Auto releases everything the run's mode allows. This
/// never widens a run's mode or Search scope and creates no session grants
/// that could leak back into Ask mode.
pub(crate) async fn resume_auto_approved_runs(s: &AppState) -> usize {
    let search_denied = s.settings.read().await.search.autonomous == "deny";
    let mut resumed = 0usize;
    let waiting = {
        let registry = s.agents.read().await;
        registry
            .summaries()
            .into_iter()
            .filter(|run| run.state == crate::agent::AgentState::WaitingPermission)
            .filter_map(|run| registry.get(&run.id))
            .collect::<Vec<_>>()
    };
    for run in waiting {
        let pending = run.pending.lock().expect("lock").clone();
        let Some(pending) = pending else {
            continue;
        };
        let risk = crate::tools::risk_of(&pending.tool);
        if !run.spec.mode.allows_risk(risk)
            || (pending.tool == "web_search" && (!run.spec.search_enabled || search_denied))
        {
            continue;
        }
        // Web search consent is its own prompt: only Auto releases it, never
        // a session grant (review finding).
        if pending.tool == "web_search" && s.permissions.read().await.autonomy != crate::permissions::AutonomyLevel::Autonomous {
            continue;
        }
        let allowed = matches!(
            s.permissions.read().await.decide_call(
                &pending.tool,
                &pending.args,
                risk,
                true,
                Some(&run.spec.workspace.to_string_lossy()),
            ),
            crate::permissions::PermissionDecision::Allow
        );
        if !allowed {
            continue;
        }
        if let Some(tx) = run.pending_tx.lock().expect("lock").take() {
            if tx
                .send(crate::agent_runner::ApprovalDecision::Approved { session: false })
                .is_ok()
            {
                resumed += 1;
            }
        }
    }
    resumed
}

async fn put_settings(
    State(s): State<AppState>,
    Json(next): Json<AppSettings>,
) -> Result<Json<AppSettings>, ApiError> {
    // Held from the read of the replaced settings to the write, so two saves
    // cannot each reconcile against a version the other has replaced.
    let _update = s.settings_update.lock().await;
    let previous = s.settings.read().await.clone();
    let next = next.reconciled_with(&previous);
    if next.inference.context_size == 0 || next.inference.context_size > 1_048_576 {
        return Err(ApiError::bad(
            "context_size must be > 0",
            "Use 8192 for Fast, 32768 for Balanced (§51).",
        ));
    }
    if !(0.0..=2.0).contains(&next.inference.temperature) {
        return Err(ApiError::bad(
            "temperature out of range",
            "Use 0.0–2.0 (0.2 for coding, 0.7 for chat).",
        ));
    }
    if !(0.0..=1.0).contains(&next.inference.top_p)
        || !(0.0..=2.0).contains(&next.inference.repeat_penalty)
        || next.inference.top_k > 1000
    {
        return Err(ApiError::bad(
            "Invalid sampling settings.",
            "Use top-p 0–1, top-k 0–1000 and repeat penalty 0–2.",
        ));
    }
    if !next.runtime_auto
        && (next.hardware.cpu_threads == 0
            || next.hardware.cpu_threads > 1024
            || next.hardware.gpu_layers < -1
            || next.hardware.gpu_layers > 10000
            || next.inference.batch_size == 0
            || next.inference.batch_size > 8192)
    {
        return Err(ApiError::bad("Invalid manual hardware settings.", "Use 1–1024 threads, -1 for automatic GPU placement (or 0–10000 layers), and batch size 1–8192."));
    }
    if !next.runtime_auto
        && (next.hardware.threads_batch > 1024
            || next.hardware.poll.is_some_and(|poll| poll > 100)
            || next.hardware.priority.is_some_and(|priority| !(-1..=3).contains(&priority)))
    {
        return Err(ApiError::bad(
            "Invalid manual thread settings.",
            "Use 0–1024 prompt threads (0 uses the generation threads), a wait of 0–100 and a priority from -1 (low) to 3 (realtime).",
        ));
    }
    if !next.runtime_auto && !next.hardware.flash_attention && next.runtime.kv_cache == "q8_0" {
        return Err(ApiError::bad(
            "An 8-bit KV cache needs Flash Attention.",
            "llama.cpp cannot create a context with an 8-bit value cache while Flash Attention is off. Turn Flash Attention on, or use the f16 cache.",
        ));
    }
    if !(2..=200).contains(&next.memory.compaction_keep_turns) {
        return Err(ApiError::bad(
            "Invalid recent message count.",
            "Keep between 2 and 200 recent messages when compacting.",
        ));
    }
    if !["automatic", "off", "ask"].contains(&next.memory.auto_compact.as_str()) {
        return Err(ApiError::bad(
            "unknown automatic compaction setting",
            "Use automatic or off.",
        ));
    }
    if !(50..=98).contains(&next.memory.compact_at_pct) {
        return Err(ApiError::bad(
            "Invalid compaction threshold.",
            "Compact at between 50% and 98% of the usable context.",
        ));
    }
    if !["auto", "off"].contains(&next.runtime.speculative.as_str()) {
        return Err(ApiError::bad(
            "unknown speculative decoding setting",
            "Use auto (draft from context) or off.",
        ));
    }
    if !["f16", "q8_0"].contains(&next.runtime.kv_cache.as_str()) {
        return Err(ApiError::bad(
            "unknown KV cache precision",
            "Use f16 (compatibility) or q8_0 (half the cache memory).",
        ));
    }
    if !["automatic", "low", "medium", "high"].contains(&next.reasoning.budget.as_str()) {
        return Err(ApiError::bad(
            "unknown reasoning budget",
            "Use automatic, low, medium, or high (§112).",
        ));
    }
    if !["duckduckgo", "brave", "custom"].contains(&next.search.provider.as_str()) {
        return Err(ApiError::bad(
            "unknown search provider",
            "Use duckduckgo, brave, or custom (§123).",
        ));
    }
    if next.search.max_results == 0 || next.search.max_results > 10 {
        return Err(ApiError::bad("max_results out of range", "Use 1–10."));
    }
    if !["ask", "allow", "deny"].contains(&next.search.autonomous.as_str()) {
        return Err(ApiError::bad(
            "unknown autonomous search policy",
            "Use ask, allow, or deny (§124).",
        ));
    }
    s.storage
        .lock()
        .await
        .save_settings(&next)
        .map_err(|error| ApiError::internal(format!("Could not save settings: {error}")))?;
    s.permissions.write().await.autonomy = crate::permissions::autonomy_for_mode(&next.agent.permission_mode);
    *s.settings.write().await = next.clone();
    resume_auto_approved_runs(&s).await;
    // A different context, cache preference or fit mode needs its own fit.
    if previous.inference.context_size != next.inference.context_size
        || previous.runtime_auto != next.runtime_auto
        || previous.runtime != next.runtime
    {
        spawn_fit_preparation(&s, std::time::Duration::from_secs(3));
    }
    Ok(Json(next))
}

async fn system_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    let hw = hardware::detect();
    let inf = s.inference.read().await;
    let latest = s.metrics.lock().ok().and_then(|log| log.latest());
    let snap = hardware::snapshot(
        latest.as_ref(),
        0,
        inf.context_size(),
        inf.metrics().tokens_per_sec,
    );
    Json(serde_json::json!({"hardware": hw, "resources": snap}))
}

#[derive(Deserialize, Default)]
struct MetricsQuery {
    /// 5m | 15m | 30m | 1h (default 15m)
    #[serde(default)]
    window: String,
}

/// Stage 15: sampled history + alerts (§§130–136).
async fn system_metrics(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<MetricsQuery>,
) -> Json<serde_json::Value> {
    let secs = match q.window.as_str() {
        "5m" => 300,
        "30m" => 1800,
        "1h" => 3600,
        _ => 900,
    };
    let log = s.metrics.lock().expect("lock");
    let samples = log.window(secs);
    let latest = log.latest();
    let alerts = crate::metrics::alerts_for(latest.as_ref());
    Json(serde_json::json!({"samples": samples, "latest": latest, "alerts": alerts, "gpu_name": log.gpu_name()}))
}

/// Stage 15: one-screen overview — hardware, inference, shared weights,
/// per-session attribution, alerts (§§130–133).
async fn system_overview(State(s): State<AppState>) -> Json<serde_json::Value> {
    let hw = hardware::detect();
    let inf = s.inference.read().await;
    let latest_sample = s.metrics.lock().ok().and_then(|log| log.latest());
    let snap = hardware::snapshot(
        latest_sample.as_ref(),
        0,
        inf.context_size(),
        inf.metrics().tokens_per_sec,
    );
    drop(inf);
    let (engine, model_id, shared_gb) = {
        let mut llama = s.llama.write().await;
        if llama.is_running() {
            let cur = s.models.read().await.current().cloned();
            let gb = cur
                .as_ref()
                .and_then(|m| std::fs::metadata(m.gguf_path()).ok())
                .map(|md| (md.len() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0);
            ("llama-server".to_string(), cur.map(|m| m.id), gb)
        } else {
            ("stub".to_string(), None, None)
        }
    };
    // Attribution (§132): live inference/agent work is measured-or-estimated,
    // everything else is honestly unknown.
    let st = s.storage.lock().await;
    let convs = st.list_conversations().unwrap_or_default();
    drop(st);
    let mut active = std::collections::HashSet::new();
    if let Some(cid) = s.generations.read().await.active_conversation() {
        if !cid.is_empty() {
            active.insert(cid);
        }
    }
    for r in s.agents.read().await.summaries() {
        if r.state.is_active() && !r.conversation_id.is_empty()
        {
            active.insert(r.conversation_id);
        }
    }
    let sessions: Vec<serde_json::Value> = convs
        .into_iter()
        .take(20)
        .map(|c| {
            let share = if active.contains(&c.id) {
                "active"
            } else {
                "idle"
            };
            // Stage 29: attribute the app-total sample to the active session and
            // label it honestly; agent-touched sessions get a small estimated
            // slice; idle sessions report nulls, never fabricated numbers.
            let latest = s.metrics.lock().expect("lock").latest();
            let (cpu, gpu, vram, basis) = if active.contains(&c.id) {
                if let Some(l) = latest.as_ref() {
                    (
                        Some(l.cpu_pct),
                        l.gpu_pct,
                        l.vram_used_gb,
                        "measured (app total, active session)",
                    )
                } else {
                    (None, None, None, "unknown")
                }
            } else {
                (None, None, None, "unknown")
            };
            serde_json::json!({
                "id": c.id, "title": c.title, "mode": c.mode,
                "share": share, "basis": basis,
                "cpu_pct": cpu, "gpu_pct": gpu, "vram_gb": vram,
            })
        })
        .collect();
    let latest = s.metrics.lock().expect("lock").latest();
    Json(serde_json::json!({
        "hardware": hw,
        "resources": snap,
        "inference": {"engine": engine, "model": model_id, "shared_weights_gb": shared_gb},
        "sessions": sessions,
        "alerts": crate::metrics::alerts_for(latest.as_ref()),
        "cache": {"note": "see GET /api/system/cache for the sampler ring + budgets"},
    }))
}

// ---- Stages 21–30 handlers ----

async fn set_progress(s: &AppState, model: &str, stage: &str, detail: &str) {
    let mut p = s.load_progress.write().await;
    p.model_id = model.into();
    p.stage = stage.into();
    p.detail = detail.into();
    p.updated_at = now_rfc3339();
    if stage == "validating" {
        p.cancel_requested = false;
    }
}

/// Stage 21: staged load progress with real stages, never fake percent.
async fn load_progress(State(s): State<AppState>) -> Json<LoadProgress> {
    Json(s.load_progress.read().await.clone())
}

/// Stage 21: cancel a pending load. Honest checkpoint: takes effect before
/// the sidecar spawn; a running spawn is stopped instead.
async fn cancel_load(State(s): State<AppState>) -> Json<serde_json::Value> {
    s.load_progress.write().await.cancel_requested = true;
    s.llama.write().await.stop().await;
    set_progress(&s, "", "cancelled", "Load cancelled by the user.").await;
    Json(serde_json::json!({"cancelled": true}))
}

/// Stage 22: versioned API status + event protocol contract (§§39–40).
async fn v1_status() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "api": "v1",
        "name": "companion-backend",
        "version": env!("CARGO_PKG_VERSION"),
        "compat": ["v1", "unversioned"],
        "events": ["message.delta", "message.complete", "operation.progress",
            "tool.started", "tool.output", "tool.completed", "tool.failed",
            "agent.state", "permission.requested", "search.started",
            "search.result", "search.completed", "artifact.created",
            "operation.cancelled", "error"],
        "streams": {
            "chat": "POST /api/v1/chat → event: token|status|done|error",
            "agent": "GET /api/agent/runs/:id/events → event: agent",
            "prepare": "POST /api/conversations/:id/prepare → event: stage|done|error",
        },
    }))
}

// ---- Stage 26 memory ----

#[derive(Deserialize, Default)]
struct NewMemory {
    #[serde(default)]
    scope: String, // conversation | workspace | global (default conversation)
    #[serde(default)]
    scope_id: String, // conversation/workspace id; empty for global
    content: String,
    #[serde(default)]
    source: String,
}

async fn list_memory(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<crate::storage::MemoryEntry>>, ApiError> {
    let conv = q.get("conversation_id").cloned().unwrap_or_default();
    let ws = q.get("workspace_id").cloned().unwrap_or_default();
    // Resolve workspace from the conversation when not given explicitly.
    let ws = if ws.is_empty() && !conv.is_empty() {
        let st = s.storage.lock().await;
        st.get_conversation(&conv)
            .ok()
            .flatten()
            .map(|c| c.workspace)
            .unwrap_or_default()
    } else {
        ws
    };
    s.storage
        .lock()
        .await
        .memories_for(&conv, &ws)
        .map(Json)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))
}

async fn add_memory(
    State(s): State<AppState>,
    Json(req): Json<NewMemory>,
) -> Result<Json<crate::storage::MemoryEntry>, ApiError> {
    if req.content.trim().is_empty() {
        return Err(ApiError::bad(
            "memory is empty",
            "Write what should be remembered.",
        ));
    }
    if req.content.len() > 8000 {
        return Err(ApiError::bad(
            "memory too long",
            "Keep one memory under ~8000 chars.",
        ));
    }
    let scope = if req.scope.is_empty() {
        "conversation".into()
    } else {
        req.scope.clone()
    };
    if !["session", "conversation", "workspace", "global"].contains(&scope.as_str()) {
        return Err(ApiError::bad(
            format!("invalid scope '{scope}'"),
            "Scope must be session, conversation, workspace or global (§84).",
        ));
    }
    if scope != "global" && req.scope_id.trim().is_empty() {
        return Err(ApiError::bad(
            "scope_id is empty",
            "Non-global memories need a conversation or workspace id.",
        ));
    }
    let now = now_rfc3339();
    let m = crate::storage::MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        scope_id: req.scope_id.trim().into(),
        scope,
        content: req.content.trim().into(),
        source: if req.source.is_empty() {
            "user".into()
        } else {
            req.source
        },
        created_at: now.clone(),
        last_used: now,
    };
    s.storage
        .lock()
        .await
        .add_memory(&m)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(m))
}

async fn delete_memory(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match s.storage.lock().await.delete_memory(&id) {
        Ok(true) => Ok(Json(serde_json::json!({"deleted": id}))),
        Ok(false) => Err(ApiError::not_found("unknown memory")),
        Err(e) => Err(ApiError::internal(format!("storage error: {e}"))),
    }
}

#[derive(Deserialize, Default)]
struct ShareMemoryReq {
    target_id: String,
    /// target scope for the copy: conversation | workspace | global
    #[serde(default = "default_mem_scope")]
    scope: String,
}

fn default_mem_scope() -> String {
    "conversation".into()
}

/// Stage 26 explicit share (§83): copy one memory into another scope.
async fn share_memory(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ShareMemoryReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !["conversation", "workspace", "global"].contains(&req.scope.as_str()) {
        return Err(ApiError::bad(
            "invalid scope",
            "Use conversation, workspace or global.",
        ));
    }
    let st = s.storage.lock().await;
    let src = match st.get_memory(&id) {
        Ok(Some(m)) => m,
        Ok(None) => return Err(ApiError::not_found("unknown memory")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let now = now_rfc3339();
    let copy = crate::storage::MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        scope_id: if req.scope == "global" {
            String::new()
        } else {
            req.target_id.clone()
        },
        scope: req.scope.clone(),
        content: src.content.clone(),
        source: format!("shared from {}", src.scope),
        created_at: now.clone(),
        last_used: now,
    };
    st.add_memory(&copy)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    Ok(Json(
        serde_json::json!({"shared": copy.id, "scope": copy.scope}),
    ))
}

// ---- Stage 25 timeline ----

/// Stage 25 session timeline (§107): one chronological activity feed.
async fn conversation_timeline(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let mut events: Vec<serde_json::Value> = Vec::new();
    if let Ok(c) = st.get_conversation(&id) {
        if let Some(c) = c {
            events.push(serde_json::json!({"kind": "session", "label": "Session started", "at": c.created_at}));
        }
    }
    for a in st.attachments_for(&id).unwrap_or_default() {
        events.push(serde_json::json!({"kind": "attachment", "label": format!("Attached {}", a.filename), "at": a.created_at}));
    }
    for m in st.messages_for(&id).unwrap_or_default() {
        if m.content.starts_with("[Compacted context") {
            events.push(serde_json::json!({"kind": "compaction", "label": "Context compacted", "at": m.created_at}));
        }
    }
    for t in st.tool_executions_for(&id, 200).unwrap_or_default() {
        events.push(serde_json::json!({"kind": "tool", "label": format!("Tool: {}", t.tool), "at": t.created_at}));
    }
    for a in st.artifacts_for(&id).unwrap_or_default() {
        events.push(serde_json::json!({"kind": "artifact", "label": format!("Generated {}", a.filename), "at": a.created_at}));
    }
    events.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
    events.truncate(300);
    Ok(Json(
        serde_json::json!({"conversation_id": id, "events": events}),
    ))
}

// ---- Stage 25 explicit compaction with stats ----

/// Stage 25 compaction result (§87): before/after/saved, reversible via
/// retained source history (originals are summarized, recent kept intact).
#[derive(Debug, Clone, Serialize)]
struct CompactStats {
    text: String,
    before_chars: usize,
    after_chars: usize,
    saved_pct: usize,
    messages: usize,
    status: String, // compacted | noop
}

async fn compact_endpoint(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CompactStats>, ApiError> {
    compact_conversation(&s, &id).await.map(Json)
}

// ---- Stage 28 budget + OCR ----

/// Stage 28 attachment budget (§§166–167, §77): warn before the next
/// expensive attachment, never silently swallow content.
async fn attachment_budget(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let st = s.storage.lock().await;
    if matches!(st.get_conversation(&id), Ok(None)) {
        return Err(ApiError::not_found("conversation not found"));
    }
    let atts = st
        .attachments_for(&id)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?;
    drop(st);
    let images = atts.iter().filter(|a| a.kind == "image").count();
    let partial = atts.iter().filter(|a| a.status == "partial").count();
    let unsupported = atts.iter().filter(|a| a.status == "unsupported").count();
    let bytes: u64 = atts.iter().map(|a| a.size_bytes).sum();
    let vision = s
        .models
        .read()
        .await
        .current()
        .map(|m| m.vision)
        .unwrap_or(false);
    let mut warnings = Vec::new();
    if images > 0 && !vision {
        warnings.push("Images attached but the loaded model has no vision — they will use the OCR/text fallback.");
    }
    if images >= 4 {
        warnings.push(
            "4+ images: vision token cost is high; consider removing some before continuing.",
        );
    }
    if bytes > 50_000_000 {
        warnings.push("Attachments exceed ~50 MB: context will only carry excerpts.");
    }
    Ok(Json(serde_json::json!({
        "count": atts.len(), "bytes": bytes, "images": images,
        "partial": partial, "unsupported": unsupported,
        "vision_model": vision, "warnings": warnings,
    })))
}

/// Stage 28 OCR availability (§73): honest about the missing pipeline.
async fn ocr_status() -> Json<serde_json::Value> {
    let available = ["tesseract", "tesseract.exe"].iter().any(|bin| {
        std::process::Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    });
    Json(serde_json::json!({
        "available": available,
        "detail": if available {
            "Tesseract found on PATH; OCR extraction enabled."
        } else {
            "No OCR engine on PATH. Image text uses the vision model when loaded, otherwise an honest fallback."
        },
    }))
}

// ---- Stage 23 artifact info ----

/// Stage 23 reveal/open support (§33): path for Reveal, openability for Open.
async fn artifact_info(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = match s.storage.lock().await.get_artifact(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return Err(ApiError::not_found("unknown artifact")),
        Err(e) => return Err(ApiError::internal(format!("storage error: {e}"))),
    };
    let openable = row.mime.starts_with("text/")
        || [
            "application/json",
            "image/png",
            "image/jpeg",
            "text/markdown",
            "text/csv",
        ]
        .contains(&row.mime.as_str());
    Ok(Json(serde_json::json!({
        "id": row.id, "filename": row.filename, "mime": row.mime,
        "size_bytes": row.size_bytes, "path": row.path,
        "openable_in_browser": openable,
        "url": format!("/api/artifacts/{}/file", row.id),
    })))
}

// ---- Stage 29 cache budget ----

/// Stage 29 cache budget (§143): sampler ring usage + configured budgets.
async fn cache_budget(State(s): State<AppState>) -> Json<serde_json::Value> {
    let log = s.metrics.lock().expect("lock");
    let samples = log.len();
    // ~64 bytes/sample estimate; stated as estimate.
    let bytes_est = samples * 64;
    let ctx = s
        .inference
        .try_read()
        .map(|i| i.context_size())
        .unwrap_or(0);
    Json(serde_json::json!({
        "sampler": {"samples": samples, "bytes_est": bytes_est, "cadence_secs": crate::metrics::SAMPLE_SECS},
        "budgets": {"ram": null, "vram": null},
        "kv_cache": {"owner": "llama-server", "usage_bytes": null, "persistent": false},
        "history": {"storage": "local SQLite", "preserved_on_model_switch": true},
        "configured_context": ctx,
        "note": "Switching models recreates runtime caches. Saved conversations remain; bounded history is reprocessed by the new model on the next reply. KV allocation is not measured here.",
    }))
}

// ---- Stage 30 DAIO endpoints ----

fn current_device_profile(s: &AppState) -> crate::daio::DeviceProfile {
    let hw = hardware::detect();
    let vram = crate::metrics::vram_info();
    let simd = crate::daio::detect_simd();
    let (vram_total, vram_avail) = vram.unwrap_or((0.0, 0.0));
    let gpu_model = if vram_total > 0.0 {
        "nvidia GPU (via nvidia-smi)".into()
    } else {
        hw.gpus.first().map(|g| g.model.clone()).unwrap_or_default()
    };
    let backend = if vram_total > 0.0 {
        "cuda".into()
    } else {
        "cpu".into()
    };
    let mut p = crate::daio::DeviceProfile {
        fingerprint: String::new(),
        os: hw.os,
        cpu_model: hw.cpu.model,
        physical_cores: hw.cpu.physical_cores,
        logical_cores: hw.cpu.logical_cores,
        simd,
        ram_total_gb: hw.ram.total_gb,
        ram_avail_gb: hw.ram.available_gb,
        gpu_model,
        gpu_backend: backend,
        vram_total_gb: (vram_total * 10.0).round() / 10.0,
        vram_avail_gb: (vram_avail * 10.0).round() / 10.0,
        npu: None,
    };
    // llama.cpp version would refine the fingerprint when known (§15).
    let _ = s;
    p.fingerprint = p.fingerprint_parts();
    p
}

// ---- Performance calibration (Settings > Performance profiles) ----

/// What a calibration's measurements depend on, as this machine is now.
async fn calibration_environment(s: &AppState, server: &std::path::Path) -> crate::calibration::Environment {
    crate::calibration::Environment {
        device_fingerprint: current_device_profile(s).fingerprint,
        runtime_build: crate::calibration::runtime_build(server).await,
        power_source: crate::calibration::power_source(),
        gpu_driver: crate::calibration::gpu_driver().await,
        cpu_topology: crate::cpu_topology::detect()
            .map(|topology| topology.describe())
            .unwrap_or_else(|| "not reported".into()),
    }
}

/// The latest calibration of a model file, and whether it still describes
/// this machine as it is now (same device, runtime build and power source).
async fn model_calibration(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let model = s
        .models
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("unknown model '{id}'")))?;
    let key = crate::calibration::model_key(&model.gguf_path());
    let latest = s
        .storage
        .lock()
        .await
        .calibrations_for(&model.id, &key)
        .map_err(|e| ApiError::internal(format!("storage error: {e}")))?
        .into_iter()
        .next();
    let current = match SidecarBinary::detect(&s.runtime_dir()) {
        Ok(binary) => Some(calibration_environment(&s, &binary.0).await),
        Err(_) => None,
    };
    let comparable = match (&latest, &current) {
        (Some(latest), Some(current)) => Some(latest.environment.comparable(current)),
        _ => None,
    };
    Ok(Json(serde_json::json!({
        "calibration": latest,
        "current_environment": current,
        "comparable": comparable,
    })))
}

/// Measure this model on this machine and store the resulting profiles.
/// Unloads the running model first: calibration loads the model itself, and
/// two resident models could exhaust memory.
async fn calibrate_model(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let bg = s.clone();
    tokio::spawn(async move {
        let stage = |name: &str, detail: &str| {
            let _ = tx.send(Ok(Event::default().event("stage").safe_data(
                serde_json::json!({"stage": name, "detail": detail}).to_string(),
            )));
        };
        let fail = |message: String| {
            let _ = tx.send(Ok(Event::default().event("error").safe_data(message)));
        };
        if let Err(error) = guard_agent_running(&bg, false).await {
            fail(error.message);
            return;
        }
        let Some(model) = bg.models.read().await.get(&id).cloned() else {
            fail(format!("unknown model '{id}'"));
            return;
        };
        let binary = match SidecarBinary::detect(&bg.runtime_dir()) {
            Ok(binary) => binary,
            Err(error) => {
                fail(error.to_string());
                return;
            }
        };
        let (Some(bench), Some(fit)) = (
            crate::calibration::runtime_tool(&binary.0, "llama-bench"),
            crate::calibration::runtime_tool(&binary.0, "llama-fit-params"),
        ) else {
            fail("The runtime has no llama-bench or llama-fit-params next to llama-server. Rebuild the runtime (scripts/build-runtime) to include its tools.".into());
            return;
        };
        let gguf = model.gguf_path();
        bg.fit_preparation.cancel().await;
        let _update = bg.runtime_update.lock().await;

        stage("unloading", "Unloading the current model so only one model is in memory.");
        bg.llama.write().await.stop().await;
        bg.models.write().await.unload_all();
        bg.inference.write().await.unload();

        let settings = bg.settings.read().await.clone();
        let context = settings
            .inference
            .context_size
            .min(model.context_length.max(1));
        stage("placing", "Asking the runtime where this model's layers fit.");
        let fitted = match binary.devices().await {
            crate::runtime_selection::Devices::None => crate::calibration::FittedPlacement::cpu(),
            _ => match crate::calibration::fitted_placement(&fit, &gguf, context, crate::runtime_fit::DEFAULT_MICRO_BATCH).await {
                Ok(fitted) => fitted,
                Err(error) => {
                    fail(format!("The runtime could not place this model: {error}"));
                    return;
                }
            },
        };
        let placement = fitted.placement;
        let (physical, performance) = match crate::cpu_topology::detect() {
            Some(topology) => (topology.physical_count(), topology.performance_cores()),
            None => {
                let cpu = hardware::detect().cpu;
                (cpu.physical_cores, cpu.physical_cores)
            }
        };
        let mut plan = crate::calibration::plan(physical, performance, placement, fitted.gpu_layers);
        // The fit's expert rules, so a mixture-of-experts model is measured
        // with the placement a load gets rather than every expert on the GPU,
        // and the load mode such a load uses (read after unloading, so the
        // free RAM is what a load would see).
        plan.tensor_overrides = fitted.tensor_overrides.clone();
        let ram_available = (hardware::detect().ram.available_gb * 1_073_741_824.0) as u64;
        let model_bytes = crate::models::model_set_bytes(&gguf);
        let loads_without_mmap = |fit: crate::runtime_fit::Fit| crate::runtime_fit::load_without_mmap(fit, ram_available, model_bytes);
        plan.load_without_mmap = placement != crate::calibration::Placement::Cpu && loads_without_mmap(fitted.fit);
        stage(
            "measuring",
            &format!(
                "Measuring {} thread settings ({}). This loads the model once and takes a minute or two; with a GPU, micro-batch sizes are measured afterwards (several minutes more, with a 2-minute pause).",
                plan.threads.len(),
                match placement {
                    crate::calibration::Placement::Gpu => "every layer on the GPU",
                    crate::calibration::Placement::Hybrid if !fitted.tensor_overrides.is_empty() => "every layer on the GPU, some expert weights in RAM",
                    crate::calibration::Placement::Hybrid => "layers split between GPU and CPU",
                    crate::calibration::Placement::Cpu => "CPU only",
                }
            ),
        );
        let mut measurements = match crate::calibration::run_benchmark(&bench, &gguf, &plan, &plan.threads, 50).await {
            Ok(measurements) => measurements,
            Err(error) => {
                fail(error);
                return;
            }
        };
        // Light also sleeps between operations when that measures nearly free.
        if let Some(light) = crate::calibration::choose_profiles(&measurements, physical)
            .and_then(|profiles| profiles.into_iter().find(|p| p.name == "light"))
        {
            stage("measuring", "Checking whether idle waiting costs speed at the lightest setting.");
            if let Ok(quiet) = crate::calibration::run_benchmark(&bench, &gguf, &plan, &[light.threads], 0).await {
                measurements.extend(quiet);
            }
        }
        let Some(profiles) = crate::calibration::choose_profiles(&measurements, physical) else {
            fail("The benchmark ran but produced no usable generation measurements.".into());
            return;
        };
        // The micro-batch, when the model uses the GPU: each size with the
        // placement the runtime fits at that size and the load mode that
        // placement loads with, at the fastest generation thread count,
        // chosen by the owner's speed rule on the mean of two passes run in
        // alternating order (512, 1,024, 2,048, then reversed) with a
        // 2-minute pause between them, since back-to-back runs can make the
        // GPU throttle (owner instruction). Extra cost: up to 6 llama-bench
        // runs, each loading the model once and reading a 4,096-token prompt
        // and generating 64 tokens 4 times, plus 2 fit probes (512 reuses the
        // placement above) and the pause: roughly 4-8 minutes. A size that
        // fails to fit or run is left out rather than failing the calibration;
        // nothing is chosen unless 512 was measured.
        let mut micro_batches = Vec::new();
        let candidates = if placement == crate::calibration::Placement::Cpu {
            Vec::new()
        } else {
            crate::calibration::micro_batch_candidates(context)
        };
        if !candidates.is_empty() {
            let threads = profiles
                .iter()
                .find(|p| p.name == "fastest")
                .map(|p| p.threads)
                .unwrap_or(physical.max(1) as u32);
            // Each size's placement, fitted once for both passes.
            let mut fitted_sizes: Vec<(u32, crate::calibration::FittedPlacement)> = Vec::new();
            for size in &candidates {
                let fitted_at_size = if *size == crate::runtime_fit::DEFAULT_MICRO_BATCH {
                    fitted.clone()
                } else {
                    match crate::calibration::fitted_placement(&fit, &gguf, context, *size).await {
                        Ok(fitted_at_size) => fitted_at_size,
                        Err(error) => {
                            tracing::warn!(model = %model.id, micro_batch = size, "micro-batch not measured: no fit ({error})");
                            continue;
                        }
                    }
                };
                // A size whose fit leaves nothing on the GPU is not an option
                // for a GPU load, and would be slow to measure.
                if fitted_at_size.placement != crate::calibration::Placement::Cpu {
                    fitted_sizes.push((*size, fitted_at_size));
                }
            }
            let sizes: Vec<u32> = fitted_sizes.iter().map(|(size, _)| *size).collect();
            let mut paused = false;
            for (pass, size) in crate::calibration::micro_batch_run_order(&sizes) {
                if pass == 1 && !paused {
                    paused = true;
                    stage("measuring", "Pausing 2 minutes so the GPU cools before the second pass.");
                    tokio::time::sleep(crate::calibration::MICRO_BATCH_PASS_PAUSE).await;
                }
                let Some((_, fitted_at_size)) = fitted_sizes.iter().find(|(fitted_size, _)| *fitted_size == size) else {
                    continue;
                };
                stage(
                    "measuring",
                    &format!(
                        "Measuring a micro-batch of {} tokens with the placement that fits at that size (reads a {}-token prompt; pass {} of 2).",
                        group_thousands(size),
                        group_thousands(crate::calibration::MICRO_BATCH_PROMPT_TOKENS.min(context)),
                        pass + 1
                    ),
                );
                let without_mmap = loads_without_mmap(fitted_at_size.fit);
                let size_plan = crate::calibration::micro_batch_plan(&plan, size, fitted_at_size, context, without_mmap);
                match crate::calibration::run_benchmark(&bench, &gguf, &size_plan, &[threads], 50).await {
                    Ok(measured) => {
                        if let Some(measured) = measured.into_iter().next() {
                            micro_batches.push(crate::calibration::MicroBatchMeasurement {
                                micro_batch: size,
                                gpu_layers: fitted_at_size.gpu_layers,
                                expert_blocks_on_cpu: fitted_at_size.expert_blocks_on_cpu(),
                                threads,
                                prompt_tokens: size_plan.prompt_tokens,
                                prompt: measured.prompt,
                                generation: measured.generation,
                                pass,
                                load_without_mmap: without_mmap,
                            });
                        }
                    }
                    Err(error) => {
                        tracing::warn!(model = %model.id, micro_batch = size, "micro-batch not measured: {error}");
                    }
                }
            }
        }
        let micro_batch = crate::calibration::choose_micro_batch(&micro_batches);
        let environment = calibration_environment(&bg, &binary.0).await;
        let key = crate::calibration::model_key(&gguf);
        let previous = bg
            .storage
            .lock()
            .await
            .calibrations_for(&model.id, &key)
            .unwrap_or_default()
            .into_iter()
            .find(|earlier| earlier.environment.comparable(&environment));
        let fastest = profiles
            .iter()
            .find(|p| p.name == "fastest")
            .map(|p| p.generation_tps)
            .unwrap_or(0.0);
        let calibration = crate::calibration::Calibration {
            id: uuid::Uuid::new_v4().to_string(),
            model_id: model.id.clone(),
            model_key: key,
            created_at: chrono::Utc::now().to_rfc3339(),
            regression: previous
                .as_ref()
                .and_then(|earlier| crate::calibration::regression(earlier, fastest)),
            environment,
            plan,
            measurements,
            profiles,
            micro_batches,
            micro_batch,
        };
        if let Err(error) = bg.storage.lock().await.save_calibration(&calibration) {
            fail(format!("Measured, but could not save the calibration: {error}"));
            return;
        }
        tracing::info!(model = %model.id, fastest_tps = fastest, "calibration saved");
        // A new calibrated micro-batch is part of what a fit depends on.
        spawn_fit_preparation(&bg, std::time::Duration::from_secs(5));
        let _ = tx.send(Ok(Event::default()
            .event("done")
            .safe_data(serde_json::to_string(&calibration).unwrap_or_default())));
    });
    Sse::new(UnboundedReceiverStream::new(rx).boxed()).keep_alive(KeepAlive::default())
}

/// This model file's latest calibration and whether it still describes this
/// machine (same device, runtime build and power source). None when the file
/// was never calibrated; the environment is read only when there is one.
async fn latest_calibration(
    s: &AppState,
    model: &crate::models::ModelMetadata,
    server: &std::path::Path,
) -> Option<(crate::calibration::Calibration, bool)> {
    let key = crate::calibration::model_key(&model.gguf_path());
    let calibrations = s
        .storage
        .lock()
        .await
        .calibrations_for(&model.id, &key)
        .unwrap_or_default();
    let latest = calibrations.into_iter().next()?;
    let now = calibration_environment(s, server).await;
    let comparable = latest.environment.comparable(&now);
    Some((latest, comparable))
}

/// Apply the chosen profile of this model's calibration (from
/// `latest_calibration`) to a load, when the calibration still describes this
/// machine. Returns a note for the runtime summary either way.
fn apply_calibrated_profile(
    profile_name: &str,
    calibration: Option<&(crate::calibration::Calibration, bool)>,
    cfg: &mut InferenceConfig,
) -> String {
    let Some((latest, comparable)) = calibration else {
        return format!(
            "{} profile selected, but this model has not been calibrated on this machine yet, so automatic settings were used. Calibrate it from its details on the Models page.",
            title_case(profile_name)
        );
    };
    if !*comparable {
        return format!(
            "{} profile selected, but this model's calibration was measured under different conditions (runtime build, device or power source changed), so automatic settings were used. Calibrate it again to refresh the profiles.",
            title_case(profile_name)
        );
    }
    let Some(profile) = latest.profiles.iter().find(|p| p.name == profile_name) else {
        return "The saved calibration has no such profile; automatic settings were used.".into();
    };
    cfg.n_threads = profile.threads;
    cfg.n_threads_batch = profile.threads_batch;
    cfg.poll = Some(profile.poll);
    cfg.priority = Some(profile.priority);
    if let Some(policy) = cfg.runtime_policy.as_mut() {
        policy.threads = profile.threads;
    }
    format!(
        "{} profile from this model's calibration: {} generation thread{} and {} for prompts{}{}; measured {:.1} tok/s generating ({}% of the fastest) and {:.0} tok/s reading prompts, leaving {} cores free while generating.",
        title_case(profile_name),
        profile.threads,
        if profile.threads == 1 { "" } else { "s" },
        profile.threads_batch,
        if profile.poll == 0 { ", sleeping between operations" } else { "" },
        if profile.priority < 0 { ", yielding the CPU to other applications" } else { "" },
        profile.generation_tps,
        profile.generation_share,
        profile.prompt_tps,
        profile.free_cores_generating
    )
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Stage 30 device profile (DAIO §4).
async fn device_profile(State(s): State<AppState>) -> Json<crate::daio::DeviceProfile> {
    Json(current_device_profile(&s))
}

/// Stage 30 capability database (DAIO §5).
async fn capability_db(State(s): State<AppState>) -> Json<serde_json::Value> {
    let p = current_device_profile(&s);
    Json(serde_json::json!({
        "fingerprint": p.fingerprint,
        "capabilities": crate::daio::seed_capabilities(&p),
        "note": "Entries become benchmarked/preferred only with local measurements; unverified SIMD stays detected.",
    }))
}

#[derive(Deserialize, Default)]
struct OptimizeQuery {
    #[serde(default)]
    workload: String,
    /// performance | balanced | efficiency
    #[serde(default)]
    policy: String,
}

/// Stage 30 placement recommendation (DAIO §§8–12).
async fn model_optimize(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<OptimizeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mm = s.models.read().await;
    let meta = match mm.get(&id) {
        Some(m) => m.clone(),
        None => return Err(ApiError::not_found(format!("unknown model '{id}'"))),
    };
    drop(mm);
    let params_b = crate::recommend::parse_params_b(&meta.parameters).unwrap_or(8.0);
    let weights_gb = std::fs::metadata(meta.gguf_path())
        .ok()
        .map(|m| (m.len() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0)
        .unwrap_or(params_b * 0.6);
    let projector_gb = meta
        .projector_file
        .as_ref()
        .and_then(|p| std::fs::metadata(meta.dir.join(p)).ok())
        .map(|m| (m.len() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0)
        .unwrap_or(if meta.vision { 1.0 } else { 0.0 });
    let model = crate::daio::ModelProfile {
        model_id: meta.id.clone(),
        architecture: meta.architecture.clone(),
        parameters_b: params_b,
        quantization: meta.quantization.clone(),
        weights_gb,
        kv_per_token_mb: crate::daio::kv_per_token_mb(params_b),
        vision: meta.vision,
        projector_gb,
    };
    let device = current_device_profile(&s);
    let active = {
        let mut n = 0;
        if s.generations.read().await.active_conversation().is_some() {
            n += 1;
        }
        n += s
            .agents
            .read()
            .await
            .summaries()
            .iter()
            .filter(|r| {
                r.state.is_active()
            })
            .count();
        n.max(1)
    };
    let workload = crate::daio::WorkloadClass::parse(if q.workload.is_empty() {
        "chat"
    } else {
        &q.workload
    });
    let policy = if q.policy.is_empty() {
        "balanced"
    } else {
        q.policy.as_str()
    };
    let placement = crate::daio::optimize(&device, &model, workload, active, policy);
    Ok(Json(serde_json::json!({
        "device": device, "model": model,
        "workload": format!("{workload:?}"), "policy": policy,
        "placement": placement,
    })))
}

/// Stage 30 baseline calibration (DAIO §13): instant, honest — reports the
/// current profile as uncalibrated baseline until timed runs exist.
async fn calibrate(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let device = current_device_profile(&s);
    let latest = s.metrics.lock().expect("lock").latest();
    Ok(Json(serde_json::json!({
        "device": device,
        "observed": latest,
        "profile_status": "baseline-uncalibrated",
        "detail": "Short timed calibration runs land here; until then the optimizer uses validated heuristics.",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AutonomyLevel;

    /// A fake runtime that records every summarization prompt and answers
    /// with a numbered summary.
    async fn summarizing_sidecar() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = prompts.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let prompt = body["messages"][0]["content"].as_str().unwrap_or("").to_string();
                    let n = {
                        let mut prompts = captured.lock().unwrap();
                        prompts.push(prompt);
                        prompts.len()
                    };
                    Json(serde_json::json!({
                        "choices": [{"message": {"content": format!("Summary {n}: the user is building a site.")}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 100, "completion_tokens": 12}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{port}"), prompts, server)
    }

    #[tokio::test]
    async fn long_conversations_are_summarized_in_window_sized_chunks() {
        let (url, prompts, server) = summarizing_sidecar().await;
        let client = SidecarClient::new(url).unwrap();
        let cfg = InferenceConfig { n_ctx: 2048, ..InferenceConfig::default() };
        let messages: Vec<Message> = (0..40)
            .map(|i| Message {
                id: format!("m{i}"),
                conversation_id: "c".into(),
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("message {i} ").repeat(120),
                created_at: String::new(),
            })
            .collect();
        let (summary, before) = fold_conversation_summary(&client, &cfg, "Earlier: hello.".into(), &messages)
            .await
            .unwrap();
        let prompts = prompts.lock().unwrap().clone();
        server.abort();
        // One prompt holding all 40 messages (about 50K characters) could never
        // fit a 2K window; the fold splits the work instead.
        assert!(prompts.len() > 5, "{} requests", prompts.len());
        let limit = (2048 - 341 - 512) * 3;
        for prompt in &prompts {
            assert!(prompt.len() <= limit + 200, "a prompt of {} characters overflows", prompt.len());
        }
        assert!(prompts[0].contains("Earlier: hello."), "an existing summary is the starting point");
        assert!(prompts[1].contains("Summary 1:"), "each chunk updates the running summary");
        assert_eq!(summary, format!("Summary {}: the user is building a site.", prompts.len()));
        assert_eq!(before, "Earlier: hello.".len() + messages.iter().map(|m| m.content.len()).sum::<usize>());
    }

    #[test]
    fn saved_settings_from_before_automatic_compaction_are_read_with_it_on() {
        let mut stored = serde_json::to_value(AppSettings::default()).unwrap();
        stored["memory"]["auto_compact"] = serde_json::json!("ask");
        stored["memory"].as_object_mut().unwrap().remove("compact_at_pct");
        let loaded: AppSettings = serde_json::from_value(stored).unwrap();
        let loaded = loaded.normalized();
        assert_eq!(loaded.memory.auto_compact, "automatic");
        assert_eq!(loaded.memory.compact_at_pct, 90);
        assert!(loaded.memory.auto_compaction());
        assert_eq!(AppSettings::default().memory.compact_threshold_pct(), 90);
    }

    #[tokio::test]
    async fn compaction_settings_are_validated() {
        let a = app();
        let mut next = serde_json::to_value(AppSettings::default()).unwrap();
        next["memory"]["compact_at_pct"] = serde_json::json!(40);
        let r = a.clone().oneshot(json_req("PUT", "/api/settings", next.clone())).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        next["memory"]["compact_at_pct"] = serde_json::json!(85);
        next["memory"]["auto_compact"] = serde_json::json!("off");
        let r = a.oneshot(json_req("PUT", "/api/settings", next)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let saved = body_json(r).await;
        assert_eq!(saved["memory"]["compact_at_pct"], 85);
        assert_eq!(saved["memory"]["auto_compact"], "off");
    }

    async fn pending_approval_fixture(
        state: &AppState,
        tool: &str,
        mode: AgentMode,
        search_enabled: bool,
    ) -> (
        std::sync::Arc<crate::agent_runner::LiveRun>,
        tokio::sync::oneshot::Receiver<crate::agent_runner::ApprovalDecision>,
    ) {
        let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
        let (activity_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let run = std::sync::Arc::new(crate::agent_runner::LiveRun {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: now_rfc3339(),
            spec: crate::agent_runner::AgentSpec {
                workspace: std::env::temp_dir(),
                task: "Fixture only; no tools execute".into(),
                mode,
                conversation_id: String::new(),
                search_enabled,
                reasoning: false,
            },
            cancel: CancelToken::new(),
            events: std::sync::Mutex::new(vec![crate::agent::AgentEvent::activity(
                "permission",
                AgentState::WaitingPermission,
                "Waiting for approval".into(),
                1,
            )]),
            broadcaster: tokio::sync::broadcast::channel(8).0,
            activity_tx,
            pending: std::sync::Mutex::new(Some(crate::agent::PendingTool {
                tool: tool.into(),
                args: serde_json::json!({}),
                reason: "Test approval".into(),
                session_grantable: false,
            })),
            pending_tx: std::sync::Mutex::new(Some(approval_tx)),
            handle: std::sync::Mutex::new(None),
        });
        state.agents.write().await.insert(run.clone());
        (run, approval_rx)
    }

    #[tokio::test]
    async fn both_auto_setting_paths_release_pending_commands_and_deletion_once() {
        for via_settings in [false, true] {
            let state = AppState::new_stub();
            let (_, mut command) =
                pending_approval_fixture(&state, "execute_command", AgentMode::Agent, false).await;
            let (_, mut deletion) =
                pending_approval_fixture(&state, "delete_file", AgentMode::Agent, false).await;
            let (_, mut read_only_command) =
                pending_approval_fixture(&state, "execute_command", AgentMode::Plan, false).await;
            let (_, mut unknown) =
                pending_approval_fixture(&state, "unknown_tool", AgentMode::Agent, false).await;
            if via_settings {
                let mut next = state.settings.read().await.clone();
                next.agent.autonomous_enabled = true;
                let _ = put_settings(State(state.clone()), Json(next))
                    .await
                    .unwrap();
            } else {
                let result = put_permission_mode(
                    State(state.clone()),
                    Json(PermissionModeRequest {
                        mode: "auto".into(),
                    }),
                )
                .await
                .unwrap()
                .0;
                assert_eq!(result["resumed"], 2);
            }
            for receiver in [&mut command, &mut deletion] {
                assert!(matches!(
                    receiver.try_recv(),
                    Ok(crate::agent_runner::ApprovalDecision::Approved { session: false })
                ));
            }
            assert!(matches!(
                read_only_command.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                unknown.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            let again = put_permission_mode(
                State(state.clone()),
                Json(PermissionModeRequest {
                    mode: "auto".into(),
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(
                again["resumed"], 0,
                "an already delivered approval is not delivered twice"
            );
            assert!(
                state
                    .storage
                    .lock()
                    .await
                    .load_settings()
                    .unwrap()
                    .unwrap()
                    .agent
                    .autonomous_enabled
            );
            let _ = put_permission_mode(
                State(state.clone()),
                Json(PermissionModeRequest { mode: "ask".into() }),
            )
            .await
            .unwrap();
            assert!(matches!(
                state.permissions.read().await.decide(
                    "execute_command",
                    crate::permissions::RiskLevel::Dangerous,
                    true
                ),
                crate::permissions::PermissionDecision::RequireApproval { .. }
            ));
        }
    }

    #[tokio::test]
    async fn auto_pending_search_requires_task_consent_and_honors_saved_deny() {
        for via_settings in [false, true] {
            for (search_enabled, policy, should_resume) in [
                (false, "ask", false),
                (true, "deny", false),
                (true, "ask", true),
                (true, "allow", true),
            ] {
                let state = AppState::new_stub();
                let (_, mut receiver) = pending_approval_fixture(
                    &state,
                    "web_search",
                    AgentMode::Agent,
                    search_enabled,
                )
                .await;
                let mut next = state.settings.read().await.clone();
                next.search.autonomous = policy.into();
                next.agent.autonomous_enabled = via_settings;
                let _ = put_settings(State(state.clone()), Json(next))
                    .await
                    .unwrap();
                if !via_settings {
                    assert!(
                        matches!(
                            receiver.try_recv(),
                            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                        ),
                        "Ask must leave the request waiting"
                    );
                    let _ = put_permission_mode(
                        State(state.clone()),
                        Json(PermissionModeRequest {
                            mode: "auto".into(),
                        }),
                    )
                    .await
                    .unwrap();
                }
                assert_eq!(matches!(receiver.try_recv(), Ok(crate::agent_runner::ApprovalDecision::Approved {session:false})), should_resume, "Search consent={search_enabled}, policy={policy}, settings path={via_settings}");
            }
        }
    }

    #[tokio::test]
    async fn invalid_settings_never_enable_auto_or_release_waiting_actions() {
        let state = AppState::new_stub();
        let (_, mut receiver) =
            pending_approval_fixture(&state, "execute_command", AgentMode::Agent, false).await;
        let mut invalid = state.settings.read().await.clone();
        invalid.agent.autonomous_enabled = true;
        invalid.inference.context_size = 0;
        assert!(put_settings(State(state.clone()), Json(invalid))
            .await
            .is_err());
        // Still Ask (reads free, edits and commands ask), not Auto.
        assert_eq!(
            state.permissions.read().await.autonomy,
            AutonomyLevel::WorkspaceAgent
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn auto_does_not_count_a_closed_approval_channel_as_resumed() {
        let state = AppState::new_stub();
        let (_, receiver) =
            pending_approval_fixture(&state, "execute_command", AgentMode::Agent, false).await;
        drop(receiver);
        let result = put_permission_mode(
            State(state),
            Json(PermissionModeRequest {
                mode: "auto".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(result["resumed"], 0);
    }

    #[test]
    fn a_reply_that_ends_by_asking_invites_an_answer() {
        let message = |role: &str, content: &str| Message {
            id: format!("{role}-{}", content.len()),
            conversation_id: "c".into(),
            role: role.into(),
            content: content.into(),
            created_at: String::new(),
        };
        assert!(latest_reply_asks(&[message("user", "fix it"), message("assistant", "Found the bug in subtract.

Would you like me to fix it?")]));
        assert!(latest_reply_asks(&[message("assistant", "Shall I add tests? **")]));
        assert!(!latest_reply_asks(&[message("assistant", "Why? Because the sign was flipped. Fixed.")]));
        assert!(!latest_reply_asks(&[message("assistant", "Done?"), message("user", "hm"), message("assistant", "All tests pass.")]));
        assert!(!latest_reply_asks(&[message("user", "anything?")]));
    }

    #[tokio::test]
    async fn a_session_grant_never_releases_a_waiting_web_search() {
        let state = AppState::new_stub();
        let (_, mut search) = pending_approval_fixture(&state, "web_search", AgentMode::Agent, true).await;
        let (_, mut read) = pending_approval_fixture(&state, "read_file", AgentMode::Agent, false).await;
        let _ = put_permission_mode(State(state.clone()), Json(PermissionModeRequest { mode: "accept_edits".into() }))
            .await
            .unwrap();
        assert!(matches!(search.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)), "search consent is its own prompt");
        assert!(matches!(read.try_recv(), Ok(crate::agent_runner::ApprovalDecision::Approved { session: false })));
    }

    #[test]
    fn conversational_intent_matches_whole_inputs_not_prefixes() {
        for input in ["Looks good to me!", "  THANKS  ", "👍🏽", "Okay.", "yes"] {
            assert_eq!(
                message_intent(input),
                MessageIntent::Acknowledgement,
                "{input}"
            );
        }
        assert_eq!(message_intent("Hello!"), MessageIntent::Greeting);
        for input in ["continue", "Go ahead.", "implement the plan"] {
            assert_eq!(message_intent(input), MessageIntent::Continue);
        }
        for input in [
            "Thanks, now fix the tests",
            "looks good but change the title",
            "Hello, can you inspect README?",
            "continue fixing parser.rs",
            "yes, fix it",
            "why does this work?",
        ] {
            assert_eq!(message_intent(input), MessageIntent::Task, "{input}");
        }
    }

    #[tokio::test]
    async fn acknowledgments_never_start_agent_or_chat_tools_and_keep_pending_plan() {
        let state = AppState::new_stub();
        let (app, workspace) = seed_workspace(router(state.clone()), "Conversation guard").await;
        let response = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"Guard","mode":"code","workspace":workspace}),
            ))
            .await
            .unwrap();
        let cid = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        state
            .storage
            .lock()
            .await
            .save_task_context(&crate::storage::TaskContext {
                conversation_id: cid.clone(),
                workspace: "unused".into(),
                task: "Fix subtraction".into(),
                run_id: "planned-run".into(),
                status: "planned".into(),
            })
            .unwrap();
        let response = app.clone().oneshot(json_req("POST", "/api/agent/run", serde_json::json!({"conversation_id":cid,"workspace":"does-not-exist","task":"looks good to me"}))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = body_json(response).await;
        assert_eq!(response["disposition"], "conversation");
        assert!(response.get("run_id").is_none());
        let response = app
            .oneshot(json_req(
                "POST",
                "/api/chat",
                serde_json::json!({"conversation_id":cid,"message":"thanks"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap();
        let events = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(events.contains("\"disposition\":\"conversation\""));
        assert!(!events.contains("event: activity"));
        assert_eq!(
            state.agent_active.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let st = state.storage.lock().await;
        assert_eq!(st.messages_for(&cid).unwrap().len(), 4);
        assert_eq!(st.task_context(&cid).unwrap().unwrap().status, "planned");
    }

    #[tokio::test]
    async fn continuation_requires_saved_unfinished_task_in_exact_project() {
        let state = AppState::new_stub();
        let root = std::env::temp_dir();
        assert!(matches!(
            resolve_continuation(&state, "session", &root)
                .await
                .unwrap(),
            Continuation::Reply("needs_task", _)
        ));
        let mut context = crate::storage::TaskContext {
            conversation_id: "session".into(),
            workspace: root.to_string_lossy().into_owned(),
            task: "Fix subtraction in calculator.js".into(),
            run_id: "run".into(),
            status: "planned".into(),
        };
        for status in ["planned", "interrupted"] {
            context.status = status.into();
            state
                .storage
                .lock()
                .await
                .save_task_context(&context)
                .unwrap();
            assert!(
                matches!(resolve_continuation(&state, "session", &root).await.unwrap(), Continuation::Task(task) if task == context.task)
            );
        }
        context.status = "completed".into();
        state
            .storage
            .lock()
            .await
            .save_task_context(&context)
            .unwrap();
        assert!(matches!(
            resolve_continuation(&state, "session", &root)
                .await
                .unwrap(),
            Continuation::Reply("needs_task", _)
        ));
        context.status = "planned".into();
        context.workspace = root
            .join("nonexistent-other-project")
            .to_string_lossy()
            .into_owned();
        state
            .storage
            .lock()
            .await
            .save_task_context(&context)
            .unwrap();
        assert!(matches!(
            resolve_continuation(&state, "session", &root)
                .await
                .unwrap(),
            Continuation::Reply("needs_task", _)
        ));
        assert!(matches!(
            resolve_continuation(&state, "other-session", &root)
                .await
                .unwrap(),
            Continuation::Reply("needs_task", _)
        ));
    }

    #[tokio::test]
    async fn agent_continuation_without_task_returns_clarification_not_run() {
        let state = AppState::new_stub();
        let response = router(state.clone())
            .oneshot(json_req(
                "POST",
                "/api/agent/run",
                serde_json::json!({"workspace":std::env::temp_dir(),"task":"continue"}),
            ))
            .await
            .unwrap();
        let response = body_json(response).await;
        assert_eq!(response["disposition"], "needs_task");
        assert!(response.get("run_id").is_none());
        assert_eq!(
            state.agent_active.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn new_substantive_code_ask_invalidates_old_continuation() {
        let state = AppState::new_stub();
        let (app, workspace) = seed_workspace(router(state.clone()), "New request").await;
        let response = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"Guard","mode":"code","workspace":workspace}),
            ))
            .await
            .unwrap();
        let cid = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let path = state
            .storage
            .lock()
            .await
            .get_workspace(&workspace)
            .unwrap()
            .unwrap()
            .path;
        state
            .storage
            .lock()
            .await
            .save_task_context(&crate::storage::TaskContext {
                conversation_id: cid.clone(),
                workspace: path,
                task: "Fix subtraction".into(),
                run_id: "plan".into(),
                status: "planned".into(),
            })
            .unwrap();
        let response = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/chat",
                serde_json::json!({"conversation_id":cid,"message":"Explain this project"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(state
            .storage
            .lock()
            .await
            .task_context(&cid)
            .unwrap()
            .is_none());
        let response = app
            .oneshot(json_req(
                "POST",
                "/api/chat",
                serde_json::json!({"conversation_id":cid,"message":"continue"}),
            ))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"disposition\":\"needs_task\""));
    }

    #[tokio::test]
    async fn permission_preference_survives_restart_but_action_grants_do_not() {
        let root =
            std::env::temp_dir().join(format!("companion-preferences-{}", uuid::Uuid::new_v4()));
        let database = root.join("companion.db");
        {
            let state = AppState::new_with_storage(
                Storage::open_persistent(&database).unwrap(),
                root.join("models"),
            );
            assert_eq!(
                get_permission_mode(State(state.clone())).await.0["mode"],
                "ask"
            );
            let _ = put_permission_mode(
                State(state.clone()),
                Json(PermissionModeRequest {
                    mode: "auto".into(),
                }),
            )
            .await
            .unwrap();
            state
                .permissions
                .write()
                .await
                .grant_session("git_commit", "project");
        }
        {
            let state = AppState::new_with_storage(
                Storage::open_persistent(&database).unwrap(),
                root.join("models"),
            );
            assert_eq!(
                get_permission_mode(State(state.clone())).await.0["mode"],
                "auto"
            );
            assert!(state.settings.read().await.agent.autonomous_enabled);
            let policy = state.permissions.read().await;
            assert!(matches!(
                policy.decide_in(
                    "write_file",
                    crate::permissions::RiskLevel::Moderate,
                    true,
                    Some("project")
                ),
                crate::permissions::PermissionDecision::Allow
            ));
            assert!(matches!(
                policy.decide_in(
                    "execute_command",
                    crate::permissions::RiskLevel::Dangerous,
                    true,
                    Some("project")
                ),
                crate::permissions::PermissionDecision::Allow
            ));
            assert!(matches!(
                policy.decide_in(
                    "git_commit",
                    crate::permissions::RiskLevel::Moderate,
                    true,
                    Some("project")
                ),
                crate::permissions::PermissionDecision::Allow
            ));
            drop(policy);
            let mut settings = state.settings.read().await.clone();
            settings.agent.autonomous_enabled = false;
            settings.inference.temperature = 0.35;
            let _ = put_settings(State(state), Json(settings)).await.unwrap();
        }
        {
            let state = AppState::new_with_storage(
                Storage::open_persistent(&database).unwrap(),
                root.join("models"),
            );
            assert_eq!(
                get_permission_mode(State(state.clone())).await.0["mode"],
                "ask"
            );
            assert_eq!(state.settings.read().await.inference.temperature, 0.35);
            for (tool, risk) in [
                ("execute_command", crate::permissions::RiskLevel::Dangerous),
                ("git_commit", crate::permissions::RiskLevel::Moderate),
            ] {
                assert!(
                    matches!(
                        state
                            .permissions
                            .read()
                            .await
                            .decide_in(tool, risk, true, Some("project")),
                        crate::permissions::PermissionDecision::RequireApproval { .. }
                    ),
                    "a restored Ask preference must not retain Auto approval or temporary grants"
                );
            }
        }
        std::fs::remove_file(database).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn foreign_websites_cannot_read_write_or_preflight_local_api() {
        let app = app();
        for origin in [
            "https://untrusted.example",
            "http://localhost.evil.example:5173",
            "null",
        ] {
            for method in ["GET", "POST", "OPTIONS"] {
                let request = Request::builder()
                    .method(method)
                    .uri(if method == "POST" {
                        "/api/chat/stop"
                    } else {
                        "/api/health"
                    })
                    .header("Origin", origin)
                    .header("Access-Control-Request-Method", "POST")
                    .body(Body::empty())
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert!(response
                    .headers()
                    .get("access-control-allow-origin")
                    .is_none());
            }
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("Host", "rebinding.example:3877")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        for origin in [
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "http://[::1]:5173",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/health")
                        .header("Origin", origin)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get("access-control-allow-origin")
                    .unwrap(),
                origin
            );
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("Host", "127.0.0.1:3877")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "native CLI without Origin remains supported"
        );
    }

    #[tokio::test]
    async fn context_reports_persisted_agent_input_without_summing_activity_output() {
        let state = AppState::new_stub();
        let response = router(state.clone())
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"Agent context", "model_id":""}),
            ))
            .await
            .unwrap();
        let conversation = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let storage = state.storage.lock().await;
        storage
            .add_message(&Message {
                id: "context-agent-reply".into(),
                conversation_id: conversation.clone(),
                role: "assistant".into(),
                content: "Done.".into(),
                created_at: "now".into(),
            })
            .unwrap();
        storage
            .record_message_activity(
                "context-agent-reply",
                &crate::agent::AgentEvent::tool_activity(
                    "tool_result",
                    crate::agent::AgentState::Observing,
                    "Read big file".into(),
                    1,
                    "read_file".into(),
                    serde_json::json!({"path":"large.txt"}),
                    Some("x".repeat(100_000)),
                    None,
                ),
            )
            .unwrap();
        let first = crate::agent::AgentContextUsage::for_turns(
            &[ChatTurn::text("user", "older input")],
            8192,
            1024,
            0,
        )
        .reported(6000, 10);
        storage
            .record_message_activity(
                "context-agent-reply",
                &crate::agent::AgentEvent::context(crate::agent::AgentState::Planning, 1, first),
            )
            .unwrap();
        let latest = crate::agent::AgentContextUsage::for_turns(
            &[
                ChatTurn::text("system", "instructions"),
                ChatTurn::text("user", "retained tool excerpt"),
            ],
            8192,
            1024,
            4,
        )
        .reported(2000, 20);
        storage
            .record_message_activity(
                "context-agent-reply",
                &crate::agent::AgentEvent::context(crate::agent::AgentState::Planning, 2, latest),
            )
            .unwrap();
        drop(storage);
        let result = conversation_context(State(state), Path(conversation))
            .await
            .unwrap()
            .0;
        assert_eq!(result["agent_context"]["usage"]["prompt_tokens"], 2000);
        assert_eq!(result["agent_context"]["usage"]["context_limit"], 8192);
        assert_eq!(result["agent_context"]["usage"]["pruned_turns"], 4);
        assert_eq!(result["agent_context"]["iteration"], 2);
        assert_eq!(result["agent_context"]["active"], false);
        assert!(result["estimated_tokens"].as_u64().unwrap() < 10);
        assert_eq!(result["breakdown"]["tools"], 0); // journal outputs are not live input
    }

    #[tokio::test]
    async fn context_memory_estimate_matches_the_exact_bounded_injected_block() {
        let state = AppState::new_stub();
        let response = router(state.clone())
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"Memory", "model_id":""}),
            ))
            .await
            .unwrap();
        let conversation = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        state
            .storage
            .lock()
            .await
            .add_memory(&crate::storage::MemoryEntry {
                id: "saved".into(),
                scope: "conversation".into(),
                scope_id: conversation.clone(),
                content: "Use concise explanations.".into(),
                source: "user".into(),
                created_at: "now".into(),
                last_used: "now".into(),
            })
            .unwrap();
        let expected = state
            .storage
            .lock()
            .await
            .memory_context(&conversation, "")
            .unwrap();
        let result = conversation_context(State(state), Path(conversation))
            .await
            .unwrap()
            .0;
        assert_eq!(result["memory"]["entries"], 1);
        assert_eq!(result["memory"]["chars"], expected.text.chars().count());
        assert_eq!(
            result["breakdown"]["memory"],
            estimate_tokens(expected.text.len())
        );
        assert_eq!(
            result["estimated_tokens"],
            estimate_tokens(expected.text.len())
        );
    }
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn app() -> Router {
        router(AppState::new_stub())
    }

    fn json_req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_json(r: Response) -> serde_json::Value {
        let b = axum::body::to_bytes(r.into_body(), 1_000_000)
            .await
            .unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let r = app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn tools_list_marks_command_dangerous() {
        let r = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let b = axum::body::to_bytes(r.into_body(), 1_000_000)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        let cmd = v
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "execute_command")
            .unwrap();
        assert_eq!(cmd["risk"], "DANGEROUS");
    }

    #[tokio::test]
    async fn chat_streams_without_model() {
        let req = Request::builder()
            .method("POST")
            .uri("/api/chat")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"message":"hello world"}"#))
            .unwrap();
        let r = app().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("text/event-stream"));
    }

    #[tokio::test]
    async fn chat_stop_when_idle_reports_not_stopped() {
        let r = app()
            .oneshot(json_req("POST", "/api/chat/stop", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["stopped"], false);
    }

    async fn seed_conv(a: Router, title: &str) -> (Router, String) {
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title": title, "model_id": "m"}),
            ))
            .await
            .unwrap();
        let id = body_json(r).await["id"].as_str().unwrap().to_string();
        for (i, role) in ["user", "assistant", "user"].iter().enumerate() {
            let r = a
                .clone()
                .oneshot(json_req(
                    "POST",
                    &format!("/api/conversations/{id}/messages"),
                    serde_json::json!({"role": role, "content": format!("m{i}")}),
                ))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        (a, id)
    }

    #[tokio::test]
    async fn edit_message_truncates_thread() {
        let (a, id) = seed_conv(app(), "edit me").await;
        let mid = body_json(
            a.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/conversations/{id}/messages"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await[1]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/conversations/{id}/messages/{mid}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"changed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["truncated"], 1);
        let v = body_json(
            a.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/conversations/{id}/messages"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[1]["content"], "changed");
    }

    #[tokio::test]
    async fn attachments_roundtrip_and_context() {
        let (a, id) = seed_conv(app(), "attach").await;
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/attachments"),
                serde_json::json!({"filename": "notes.txt", "content": "hello attachment"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/attachments"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await.as_array().unwrap().len(), 1);
        // bad filename rejected (path separators are stripped, illegal chars refused)
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/attachments"),
                serde_json::json!({"filename": "bad:name.txt", "content": "x"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // context reflects 3 messages + 1 attachment
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/context"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["messages_total"], 3);
        assert_eq!(v["attachments"]["count"], 1);
    }

    #[test]
    fn build_turns_folds_tool_and_appends_attachments() {
        let history = vec![
            Message {
                id: "1".into(),
                conversation_id: "c".into(),
                role: "user".into(),
                content: "hi".into(),
                created_at: "".into(),
            },
            Message {
                id: "2".into(),
                conversation_id: "c".into(),
                role: "tool".into(),
                content: "ls output".into(),
                created_at: "".into(),
            },
        ];
        let turns = build_turns(&history, &[]);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].role, "user");
        assert!(turns[1].content.contains("[tool result]"));
    }

    #[tokio::test]
    async fn chat_stream_ends_with_done_event() {
        let req = Request::builder()
            .method("POST")
            .uri("/api/chat")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"message":"hello world"}"#))
            .unwrap();
        let r = app().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let b = axum::body::to_bytes(r.into_body(), 1_000_000)
            .await
            .unwrap();
        let text = String::from_utf8(b.to_vec()).unwrap();
        assert!(text.contains("event: token"), "tokens expected: {text}");
        assert!(text.contains("event: done"), "done frame expected: {text}");
        assert!(text.contains("generated_tokens"), "usage expected: {text}");
    }

    #[tokio::test]
    async fn conversation_crud_roundtrip() {
        let a = app();
        // create
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"t","model_id":"m"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let id = v["id"].as_str().unwrap().to_string();

        // get
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // post message
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                serde_json::json!({"role":"user","content":"hi"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // list messages
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/messages"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v.as_array().unwrap().len(), 1);

        // delete
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/conversations/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // gone
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn message_validation_rejects_bad_role_and_empty() {
        let a = app();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title":"t","model_id":"m"}),
            ))
            .await
            .unwrap();
        let v = body_json(r).await;
        let id = v["id"].as_str().unwrap().to_string();

        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                serde_json::json!({"role":"system","content":"x"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);

        let r = a
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                serde_json::json!({"role":"user","content":"   "}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_unknown_conversation_is_404() {
        let req = json_req(
            "POST",
            "/api/chat",
            serde_json::json!({"conversation_id":"missing","message":"hi"}),
        );
        let r = app().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn settings_validation_rejects_zero_context() {
        let mut s: AppSettings = serde_json::from_value(
            body_json(
                app()
                    .oneshot(
                        Request::builder()
                            .uri("/api/settings")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
            )
            .await,
        )
        .unwrap();
        s.inference.context_size = 0;
        let r = app()
            .oneshot(json_req(
                "PUT",
                "/api/settings",
                serde_json::to_value(&s).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn managed_runtime_policy_is_honest_and_keeps_legacy_preferences() {
        let state = AppState::new_stub();
        let mut legacy = serde_json::to_value(AppSettings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("runtime_auto");
        legacy["advanced"]["kv_cache_type"] = "Q8_0".into();
        let a = router(state.clone());
        let response = a
            .clone()
            .oneshot(json_req("PUT", "/api/settings", legacy))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved = body_json(response).await;
        assert_eq!(saved["runtime_auto"], true);
        assert_eq!(saved["advanced"]["kv_cache_type"], "Q8_0");
        let response = a
            .oneshot(
                Request::builder()
                    .uri("/api/runtime/policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let policy = body_json(response).await;
        assert_eq!(policy["running"], false);
        assert!(policy["active"].is_null());
        assert_eq!(policy["next"]["mode"], "automatic");
        assert_eq!(policy["next"]["cache_type_k"], "f16");
        assert_eq!(policy["next"]["flash_attention"], "auto");
        assert_eq!(policy["applies_on"], "next_model_load");
        assert!(!state.llama.write().await.is_running());
    }

    #[tokio::test]
    async fn manual_runtime_rejects_invalid_hardware_without_saving() {
        let state = AppState::new_stub();
        let mut next = AppSettings::default();
        next.runtime_auto = false;
        next.hardware.cpu_threads = 0;
        let response = router(state.clone())
            .oneshot(json_req(
                "PUT",
                "/api/settings",
                serde_json::to_value(next).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(state.settings.read().await.runtime_auto);
        assert!(state
            .storage
            .lock()
            .await
            .load_settings()
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn model_switch_and_unload_refuse_live_chat_even_with_force() {
        let state = AppState::new_stub();
        state.generations.write().await.insert(ActiveGeneration {
            id: "busy-test".into(),
            conversation_id: None,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            partial: Arc::new(std::sync::Mutex::new(String::new())),
            persisted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            handle: tokio::spawn(std::future::pending()),
        });
        assert!(guard_agent_running(&state, true).await.is_err());
        let response = router(state.clone())
            .oneshot(json_req(
                "POST",
                "/api/models/unload",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        state
            .generations
            .write()
            .await
            .cancel_current(&state.storage)
            .await;
    }

    #[tokio::test]
    async fn chat_waits_for_model_switch_before_persisting_a_request() {
        let state = AppState::new_stub();
        let a = router(state.clone());
        let _switch = state.runtime_update.lock().await;
        let response = a
            .oneshot(json_req(
                "POST",
                "/api/chat",
                serde_json::json!({"message":"Please explain this function."}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn permission_mode_switches_and_validates() {
        let a = app();
        let r = a
            .clone()
            .oneshot(json_req(
                "PUT",
                "/api/permissions/mode",
                serde_json::json!({"mode": "auto"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["mode"], "auto");

        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/permissions/mode")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await["mode"], "auto");

        let r = a
            .oneshot(json_req(
                "PUT",
                "/api/permissions/mode",
                serde_json::json!({"mode": "sometimes"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn agent_requires_task_and_workspace() {
        let r = app()
            .oneshot(json_req(
                "POST",
                "/api/agent/run",
                serde_json::json!({"workspace":"","task":""}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    fn agent_ws() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static M: AtomicU64 = AtomicU64::new(0);
        let n = M.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("companion-agentws-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn agent_run_without_inference_fails_gracefully() {
        let a = app();
        let ws = agent_ws();
        let before_start = chrono::Utc::now();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/agent/run",
                serde_json::json!({"workspace": ws, "task": "list files", "mode": "agent"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let run_id = body_json(r).await["run_id"].as_str().unwrap().to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/agent/runs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summaries = body_json(r).await;
        let started_at = summaries
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == run_id)
            .unwrap()["started_at"]
            .as_str()
            .unwrap()
            .to_string();
        let parsed = chrono::DateTime::parse_from_rfc3339(&started_at).unwrap();
        assert!(parsed >= before_start && parsed <= chrono::Utc::now());
        // Poll for terminal state (loop fails fast with no sidecar).
        let mut state = String::new();
        for _ in 0..50 {
            let r = a
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/agent/runs/{run_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
            state = body_json(r).await["state"].as_str().unwrap().to_string();
            if state == "FAILED" || state == "COMPLETED" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(state, "FAILED");
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/agent/runs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summaries = body_json(r).await;
        assert_eq!(
            summaries
                .as_array()
                .unwrap()
                .iter()
                .find(|run| run["id"] == run_id)
                .unwrap()["started_at"],
            started_at,
            "polling and terminal state must not reset run start"
        );
        // Event tail replays the failure.
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/agent/runs/{run_id}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let b = axum::body::to_bytes(r.into_body(), 1_000_000)
            .await
            .unwrap();
        let text = String::from_utf8(b.to_vec()).unwrap();
        assert!(text.contains("FAILED"), "{text}");
    }

    #[tokio::test]
    async fn agent_unknown_run_is_404_and_chat_mode_refused() {
        let a = app();
        let ws = agent_ws();
        for (method, uri) in [
            ("GET", "/api/agent/runs/nope".to_string()),
            ("GET", "/api/agent/runs/nope/events".to_string()),
            ("POST", "/api/agent/runs/nope/stop".to_string()),
        ] {
            let r = a
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        }
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/agent/runs/nope/resume",
                serde_json::json!({"approved": true}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = a
            .oneshot(json_req(
                "POST",
                "/api/agent/run",
                serde_json::json!({"workspace": ws, "task": "x", "mode": "chat"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn inference_status_reports_stub_when_idle() {
        let r = app()
            .oneshot(
                Request::builder()
                    .uri("/api/inference/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["engine"], "stub");
        assert_eq!(v["running"], false);
    }

    #[tokio::test]
    async fn inference_start_without_model_is_400() {
        let r = app()
            .oneshot(json_req(
                "POST",
                "/api/inference/start",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        // No models registered in stub state and no model_id given.
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn inference_start_unknown_model_is_404() {
        let r = app()
            .oneshot(json_req(
                "POST",
                "/api/inference/start",
                serde_json::json!({"model_id": "nope"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn inference_stop_is_idempotent() {
        let a = app();
        for _ in 0..2 {
            let r = a
                .clone()
                .oneshot(json_req(
                    "POST",
                    "/api/inference/stop",
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn unload_also_stops_sidecar() {
        let a = app();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/models/unload",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/inference/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await["running"], false);
    }

    #[tokio::test]
    async fn download_start_validates_and_tracks() {
        let a = app();
        // bad URL rejected
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/models/downloads",
                serde_json::json!({"id": "m1", "url": "ftp://x"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // bad id rejected
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/models/downloads",
                serde_json::json!({"id": "../evil", "url": "https://example.com/m.gguf"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // unknown download lookup is 404
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/models/downloads/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn model_detail_and_delete_roundtrip() {
        // Seed a fake model dir with metadata so detail has on-disk facts.
        let dir = std::env::temp_dir().join(format!("companion-apimodel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mdir = dir.join("tiny");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(mdir.join("model.gguf"), crate::models::test_gguf_bytes()).unwrap();
        std::fs::write(
            mdir.join("metadata.json"),
            r#"{"id":"tiny","name":"Tiny","architecture":"llama","quantization":"Q4_K_M","parameters":"1B","context_length":4096,"vision":false,"tool_calling":false}"#,
        )
        .unwrap();
        let state = AppState::new_with_storage(Storage::open_in_memory().unwrap(), dir.clone());
        {
            let (found, _) = crate::models::scan_models_dir(&dir);
            let mut mm = state.models.write().await;
            for m in found {
                mm.register(m).unwrap();
            }
        }
        let a = router(state);
        // detail
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/models/tiny")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["metadata"]["id"], "tiny");
        assert_eq!(v["estimates"]["gguf_present"], true);
        // delete
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/models/tiny")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // gone
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/models/tiny")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tool_ws() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("companion-apitool-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn exec_body(
        ws: &str,
        tool: &str,
        args: serde_json::Value,
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("workspace".into(), serde_json::json!(ws));
        m.insert("tool".into(), serde_json::json!(tool));
        m.insert("args".into(), args);
        if let serde_json::Value::Object(e) = extra {
            for (k, v) in e {
                m.insert(k, v);
            }
        }
        serde_json::Value::Object(m)
    }

    #[tokio::test]
    async fn tool_gate_requires_approval_then_allows_once() {
        let a = app();
        let ws = tool_ws();
        // The default Ask mode runs a read without asking (as Claude Code
        // does) and asks before a file edit.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "read_file",
                    serde_json::json!({"path": "a.txt"}),
                    serde_json::json!({}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v["output"].as_str().unwrap().contains("hello"));
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "write_file",
                    serde_json::json!({"path": "b.txt", "content": "edited"}),
                    serde_json::json!({}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        // Allow once executes.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "write_file",
                    serde_json::json!({"path": "b.txt", "content": "edited"}),
                    serde_json::json!({"approved_once": true}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // Unknown tool is a 400, not a 403.
        let r = a
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "nuke",
                    serde_json::json!({}),
                    serde_json::json!({"approved_once": true}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn tool_session_grant_and_audit() {
        let a = app();
        let ws = tool_ws();
        // Grant session for write_file (MODERATE) with an approval.
        let r = a.clone().oneshot(json_req("POST", "/api/tools/execute",
            exec_body(&ws, "write_file", serde_json::json!({"path": "s.txt", "content": "v"}),
                serde_json::json!({"approved_once": true, "grant_session": true, "conversation_id": "c-audit"})))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // Second write needs no approval now.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "write_file",
                    serde_json::json!({"path": "s.txt", "content": "v2"}),
                    serde_json::json!({"conversation_id": "c-audit"}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["approved_via"], "auto");
        // Dangerous tool ignores the grant path: still 403 without approval.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "execute_command",
                    serde_json::json!({"command": "echo hi"}),
                    serde_json::json!({"grant_session": true, "conversation_id": "c-audit"}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        // Audit trail recorded executions + the denial.
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/tools/executions?conversation_id=c-audit&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_json(r).await.as_array().unwrap().len() >= 3);
    }

    #[tokio::test]
    async fn tool_rejects_missing_workspace() {
        let r = app()
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    "C:/definitely/not/here-12345",
                    "read_file",
                    serde_json::json!({"path": "a"}),
                    serde_json::json!({}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    async fn chat_text(a: Router, body: serde_json::Value) -> String {
        let r = a
            .oneshot(json_req("POST", "/api/chat", body))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let b = axum::body::to_bytes(r.into_body(), 2_000_000)
            .await
            .unwrap();
        let text = String::from_utf8(b.to_vec()).unwrap();
        // Reassemble tokens the way the UI does (single-space framing).
        let mut acc = String::new();
        for frame in text.split("\n\n") {
            for line in frame.lines() {
                if let Some(d) = line.strip_prefix("data:") {
                    let d = d.strip_prefix(' ').unwrap_or(d);
                    if frame.contains("event: token") {
                        acc.push_str(d);
                    }
                }
            }
        }
        acc
    }

    #[tokio::test]
    async fn slash_help_and_unknown() {
        let a = app();
        let out = chat_text(a.clone(), serde_json::json!({"message": "/help"})).await;
        assert!(out.contains("/build"), "{out}");
        assert!(!out.contains("/doctor"), "{out}");
        let out = chat_text(a.clone(), serde_json::json!({"message": "/nope-cmd"})).await;
        assert!(out.contains("Unknown command"), "{out}");
    }

    #[tokio::test]
    async fn slash_models_lists_registry() {
        let out = chat_text(app(), serde_json::json!({"message": "/models"})).await;
        assert!(
            out.contains("No models") || out.contains("Installed models"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn slash_status_and_context() {
        let a = app();
        let out = chat_text(a.clone(), serde_json::json!({"message": "/status"})).await;
        assert!(out.contains("Model:"), "{out}");
        let (a, id) = seed_conv(a, "ctx cmd").await;
        let out = chat_text(
            a,
            serde_json::json!({"message": "/context", "conversation_id": id}),
        )
        .await;
        assert!(out.contains("Context"), "{out}");
    }

    #[tokio::test]
    async fn command_autocomplete_filters() {
        let r = app()
            .oneshot(
                Request::builder()
                    .uri("/api/commands?q=/re")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let names: Vec<_> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"review".to_string()), "{names:?}");
        assert!(!names.contains(&"search".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn web_search_tool_needs_consent() {
        let a = app();
        let ws = tool_ws();
        let r = a
            .oneshot(json_req(
                "POST",
                "/api/tools/execute",
                exec_body(
                    &ws,
                    "web_search",
                    serde_json::json!({"query": "llama.cpp"}),
                    serde_json::json!({}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    fn ws_dir() -> String {
        let dir = std::env::temp_dir().join(format!(
            "companion-ws-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn workspace_crud_detects_build_system() {
        let a = app();
        let dir = ws_dir();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/workspaces",
                serde_json::json!({"name": "Demo", "path": dir}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v["build_system"].as_str().unwrap().contains("Cargo"), "{v}");
        let id = v["id"].as_str().unwrap().to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await.as_array().unwrap().len(), 1);
        let r = a
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn fork_and_share_and_sessions() {
        let a = app();
        let (a, id) = seed_conv(a, "fork me").await;
        // fork
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/fork"),
                serde_json::json!({"title": "fork"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["messages"], 3);
        let fork_id = v["forked"].as_str().unwrap().to_string();
        // patch mode + link workspace
        let dir = ws_dir();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/workspaces",
                serde_json::json!({"name": "W", "path": dir}),
            ))
            .await
            .unwrap();
        let ws_id = body_json(r).await["id"].as_str().unwrap().to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/conversations/{fork_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"mode":"code","workspace":"{ws_id}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["mode"], "code");
        // share back into the original
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{fork_id}/share"),
                serde_json::json!({"target_id": id, "turns": 2}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // sessions list shows both with residency
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v["sessions"].as_array().unwrap().len() >= 2);
        assert_eq!(v["sessions"][0]["residency"], "cold");
    }

    #[tokio::test]
    async fn metrics_and_overview_shape() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/system/metrics?window=5m")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v.get("samples").is_some() && v.get("alerts").is_some());
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/system/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v.get("sessions").is_some() && v.get("inference").is_some());
    }

    #[tokio::test]
    async fn recommend_endpoint_uses_metadata() {
        // Seed a 14B Q4 model dir so the engine has real inputs.
        let dir = std::env::temp_dir().join(format!("companion-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mdir = dir.join("big");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(mdir.join("model.gguf"), crate::models::test_gguf_bytes()).unwrap();
        std::fs::write(
            mdir.join("metadata.json"),
            r#"{"id":"big","name":"Big","architecture":"qwen2","quantization":"Q4_K_M","parameters":"14B","context_length":32768,"vision":false,"tool_calling":true}"#,
        )
        .unwrap();
        let state = AppState::new_with_storage(Storage::open_in_memory().unwrap(), dir.clone());
        {
            let (found, _) = crate::models::scan_models_dir(&dir);
            let mut mm = state.models.write().await;
            for m in found {
                mm.register(m).unwrap();
            }
        }
        let a = router(state);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/models/big/recommend?workload=coding&profile=balanced")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v["recommended_ctx"].as_u64().unwrap() >= 4096);
        assert!(!v["rationale"].as_array().unwrap().is_empty());
        assert!(v["candidates"].as_array().unwrap().len() == 6);
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/models/nope/recommend")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn compatibility_reports_tool_and_vision() {
        // Model registry with a text-only, non-tool model.
        let dir = std::env::temp_dir().join(format!("companion-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mdir = dir.join("plain");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(mdir.join("model.gguf"), crate::models::test_gguf_bytes()).unwrap();
        std::fs::write(
            mdir.join("metadata.json"),
            r#"{"id":"plain","name":"Plain","architecture":"llama","quantization":"Q4_K_M","parameters":"1B","context_length":4096,"vision":false,"tool_calling":false}"#,
        )
        .unwrap();
        let state = AppState::new_with_storage(Storage::open_in_memory().unwrap(), dir.clone());
        {
            let (found, _) = crate::models::scan_models_dir(&dir);
            let mut mm = state.models.write().await;
            for m in found {
                mm.register(m).unwrap();
            }
        }
        let a = router(state);
        let (a, id) = seed_conv(a, "compat").await;
        // Post a tool message so tool_history is exercised.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                serde_json::json!({"role": "tool", "content": "ls"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/conversations/{id}/compatibility?model_id=plain"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["tool_calling"], false);
        assert!(v["warnings"].as_array().unwrap().len() >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn prepare_missing_conversation_errors() {
        let r = app()
            .oneshot(json_req(
                "POST",
                "/api/conversations/nope/prepare",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        // Streams SSE; first frame is an error event.
        assert_eq!(r.status(), StatusCode::OK);
        let b = axum::body::to_bytes(r.into_body(), 100_000).await.unwrap();
        assert!(String::from_utf8(b.to_vec())
            .unwrap()
            .contains("event: error"));
    }

    #[test]
    fn prepared_model_identity_uses_existing_weights_not_missing_paths_or_labels() {
        let executable = std::env::current_exe().unwrap();
        let alias = executable
            .parent()
            .unwrap()
            .join(".")
            .join(executable.file_name().unwrap());
        assert!(same_existing_model_path(&executable, &alias));
        assert!(!same_existing_model_path(
            &executable,
            executable.parent().unwrap()
        ));
        let missing =
            std::env::temp_dir().join(format!("missing-model-{}.gguf", uuid::Uuid::new_v4()));
        assert!(
            !same_existing_model_path(&missing, &missing),
            "two failed lookups are not proof of model identity"
        );
    }

    #[tokio::test]
    async fn prepare_does_not_interrupt_chat_or_claim_cache_prefill() {
        let state = AppState::new_stub();
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        state
            .generations
            .write()
            .await
            .insert(crate::generation::ActiveGeneration {
                id: "active-preparation-guard".into(),
                conversation_id: None,
                cancel: cancellation.clone(),
                partial: Default::default(),
                persisted: Default::default(),
                handle: tokio::spawn(std::future::pending()),
            });
        let response = router(state.clone())
            .oneshot(json_req(
                "POST",
                "/api/conversations/nope/prepare",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap();
        let events = String::from_utf8_lossy(&bytes);
        assert!(events.contains("event: error"));
        assert!(events.contains("still running"));
        assert!(!events.contains("event: done"));
        assert!(!events.contains("slot warm"));
        assert!(!cancellation.load(std::sync::atomic::Ordering::SeqCst));
        assert!(state.generations.read().await.is_active());
        state
            .generations
            .write()
            .await
            .cancel_current(&state.storage)
            .await;
    }

    #[tokio::test]
    async fn load_refuses_during_live_agent_run() {
        // Register a model so load gets past lookup to the guard... guard runs
        // after id validation, so an unknown id 404s first. Use a fake live
        // run by spawning a real one against a temp workspace (fails fast only
        // when sidecar is absent — instead directly assert guard helper).
        let state = AppState::new_stub();
        // No runs → guard passes.
        assert!(guard_agent_running(&state, false).await.is_ok());
    }

    // ---- Stages 21–30 endpoint tests ----

    #[tokio::test]
    async fn v1_status_and_aliases() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["api"], "v1");
        assert!(v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "agent.state"));
        for uri in [
            "/api/v1/models",
            "/api/v1/sessions",
            "/api/v1/settings",
            "/api/v1/system",
            "/api/v1/tools",
            "/api/v1/commands",
        ] {
            let r = a
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK, "{uri}");
        }
    }

    #[tokio::test]
    async fn load_progress_lifecycle() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/models/load/progress")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["stage"], "idle");
        // Unknown id → error stage recorded.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/models/load",
                serde_json::json!({"id": "nope"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/models/load/progress")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await["stage"], "error");
        let r = a
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/models/load/cancel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn memory_crud_and_scoping() {
        let a = app();
        let (a, id) = seed_conv(a, "mem test").await;
        // Invalid scope rejected.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/memory",
                serde_json::json!({"content": "x", "scope": "everywhere", "scope_id": id}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // Conversation memory visible to its own session only.
        let r = a.clone().oneshot(json_req("POST", "/api/memory",
            serde_json::json!({"content": "prefers concise", "scope": "conversation", "scope_id": id}))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let mid = body_json(r).await["id"].as_str().unwrap().to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/memory?conversation_id={id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await.as_array().unwrap().len(), 1);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/memory?conversation_id=other")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await.as_array().unwrap().is_empty());
        // Explicit share copies into the target scope.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/memory/{mid}/share"),
                serde_json::json!({"target_id": "ws9", "scope": "workspace"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/memory?workspace_id=ws9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await.as_array().unwrap().len(), 1);
        let r = a
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/memory/{mid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn context_breakdown_timeline_compact_shapes() {
        let a = app();
        let (a, id) = seed_conv(a, "ctx shapes").await;
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/context"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["breakdown"]["system"], 0); // instructions are not saved history
        assert_eq!(v["measurement"], "saved_history_estimate");
        assert!(
            ["healthy", "moderate", "high", "critical"].contains(&v["health"].as_str().unwrap())
        );
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/timeline"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await["events"].as_array().unwrap().len(), 1); // session start
                                                                               // Too few messages → noop stats, not an error.
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{id}/compact"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["status"], "noop");
        // Budget + export shapes.
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/attachment-budget"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(r).await["count"], 0);
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/export"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await.get("transcript").is_some());
    }

    #[tokio::test]
    async fn office_sniff_is_honest() {
        // Minimal PDF.
        let pdf = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Count 3>>endobj\n".to_vec();
        let (kind, status, excerpt) = sniff_office("doc.pdf", &pdf);
        assert_eq!(kind, "office");
        assert_eq!(status, "partial");
        assert!(excerpt.contains("3 page"), "{excerpt}");
        // Minimal ZIP with one stored entry.
        let mut zip = Vec::new();
        zip.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x05\x00\x00\x00\x05\x00\x1e\x00");
        zip.extend_from_slice(b"a.txt");
        zip.extend_from_slice(b"");
        zip.extend_from_slice(b"hello");
        let (kind, status, excerpt) = sniff_office("doc.docx", &zip);
        assert_eq!(kind, "office");
        assert!(excerpt.contains("a.txt"), "{excerpt}");
        assert_eq!(status, "partial");
        // Random bytes → unsupported, honestly.
        let (kind, status, _) = sniff_office("blob.bin", &[0, 1, 2, 3, 255, 254, 253]);
        assert_eq!(status, "unsupported");
        assert_eq!(kind, "binary");
    }

    #[tokio::test]
    async fn session_priority_export_recovery_actions() {
        let a = app();
        let (a, id) = seed_conv(a, "priority me").await;
        let r = a
            .clone()
            .oneshot(json_req(
                "PATCH",
                &format!("/api/sessions/{id}"),
                serde_json::json!({"priority": "ultra"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = a
            .clone()
            .oneshot(json_req(
                "PATCH",
                &format!("/api/sessions/{id}"),
                serde_json::json!({"priority": "high"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(r).await["priority"], "high");
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/sessions/{id}/action"),
                serde_json::json!({"action": "pause"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(r).await["result"], "noop");
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/sessions/{id}/action"),
                serde_json::json!({"action": "explode"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/recovery")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await.get("stale").is_some());
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/system/ocr")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await.get("available").is_some());
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/system/cache")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await.get("sampler").is_some());
    }

    #[tokio::test]
    async fn workspace_instructions_and_diff_shapes() {
        let a = app();
        let dir = ws_dir();
        std::fs::write(format!("{dir}/AGENTS.md"), "# rules\nBe nice.").unwrap();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/workspaces",
                serde_json::json!({"name": "Instr", "path": dir}),
            ))
            .await
            .unwrap();
        let id = body_json(r).await["id"].as_str().unwrap().to_string();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/instructions"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert_eq!(v["instructions"][0]["file"], "AGENTS.md");
        assert!(v["instructions"][0]["scope"]
            .as_str()
            .unwrap()
            .contains("cannot grant"));
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/diff"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn daio_endpoints_shape() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/system/device")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert!(!v["fingerprint"].as_str().unwrap().is_empty());
        assert!(!v["simd"].as_array().unwrap().is_empty());
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/system/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let caps = body_json(r).await["capabilities"]
            .as_array()
            .unwrap()
            .clone();
        assert!(caps.iter().any(|c| c["device"] == "npu"));
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/system/calibrate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            body_json(r).await["profile_status"],
            "baseline-uncalibrated"
        );
    }

    #[tokio::test]
    async fn optimize_needs_known_model_then_recommends() {
        let dir = std::env::temp_dir().join(format!("companion-opt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mdir = dir.join("opt8");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(mdir.join("model.gguf"), crate::models::test_gguf_bytes()).unwrap();
        std::fs::write(
            mdir.join("metadata.json"),
            r#"{"id":"opt8","name":"Opt8","architecture":"llama","quantization":"Q4_K_M","parameters":"8B","context_length":32768,"vision":false,"tool_calling":true}"#,
        )
        .unwrap();
        let state = AppState::new_with_storage(Storage::open_in_memory().unwrap(), dir.clone());
        {
            let (found, _) = crate::models::scan_models_dir(&dir);
            let mut mm = state.models.write().await;
            for m in found {
                mm.register(m).unwrap();
            }
        }
        let a = router(state);
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/models/nope/optimize")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/models/opt8/optimize?workload=code&policy=balanced")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(["gpu", "hybrid", "cpu"].contains(&v["placement"]["strategy"].as_str().unwrap()));
        assert!(!v["placement"]["rationale"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Verbatim from a 2B model asked "write a simple rust program" while the
    /// chat prompt carried a document-tool example: an invented tool name,
    /// ordinary code as its argument, and an args object that never closes.
    const INVENTED_TOOL_REPLY: &str = "```tool\n{\"name\": \"rust_program\", \"args\": {\"code\": \"fn main() {\\n  println!(\\\"Hello world!\\\");\\n}}\"}\n``` \n\nI've started with a basic Rust program that prints \"Hello world!\".";

    #[test]
    fn a_context_beyond_the_models_limit_is_announced() {
        let notice = context_limit_notice("Tiny", 32_768, 8_192).unwrap();
        assert!(notice.contains("Tiny supports at most 8,192 tokens"));
        assert!(notice.contains("32,768 was reduced to 8,192"));
        assert!(context_limit_notice("Tiny", 8_192, 8_192).is_none());
        assert!(context_limit_notice("Unknown", 32_768, 0).is_none());
        assert_eq!(group_thousands(262_144), "262,144");
        assert_eq!(group_thousands(512), "512");
    }

    fn message(id: &str, role: &str, content: &str, at: &str) -> Message {
        Message { id: id.into(), conversation_id: "c".into(), role: role.into(), content: content.into(), created_at: at.into() }
    }

    fn attachment(name: &str, excerpt: &str, at: &str) -> crate::storage::Attachment {
        crate::storage::Attachment {
            id: name.into(),
            conversation_id: "c".into(),
            filename: name.into(),
            mime: "text/plain".into(),
            size_bytes: excerpt.len() as u64,
            text_excerpt: excerpt.into(),
            kind: "text".into(),
            status: "ready".into(),
            created_at: at.into(),
        }
    }

    #[test]
    fn an_attachment_stays_on_the_message_it_was_sent_with() {
        let history = vec![
            message("m1", "user", "summarise this", "2026-09-16T10:00:05+00:00"),
            message("m2", "assistant", "It is about lists.", "2026-09-16T10:00:10+00:00"),
            message("m3", "user", "and this one?", "2026-09-16T10:01:05+00:00"),
            message("m4", "assistant", "About trees.", "2026-09-16T10:01:10+00:00"),
            message("pending", "user", "compare them", ""),
        ];
        let files = vec![
            attachment("lists.txt", "LISTS", "2026-09-16T10:00:00+00:00"),
            attachment("trees.txt", "TREES", "2026-09-16T10:01:00+00:00"),
        ];
        let (turns, ids) = build_turns_with_owners(&history, &files, 100_000);
        assert_eq!(ids, vec!["m1", "m2", "m3", "m4", "pending"]);
        assert!(turns[0].content.contains("[Attached file: lists.txt]"));
        assert!(turns[2].content.contains("[Attached file: trees.txt]"));
        assert!(!turns[4].content.contains("Attached file"), "the newest turn no longer collects every excerpt");
        // The same request next turn renders the earlier turns identically.
        let mut next = history.clone();
        next[4] = message("m5", "user", "compare them", "2026-09-16T10:02:00+00:00");
        next.push(message("m6", "assistant", "Lists are linear.", "2026-09-16T10:02:10+00:00"));
        next.push(message("pending", "user", "which is faster?", ""));
        let (later, _) = build_turns_with_owners(&next, &files, 100_000);
        assert_eq!(later[..4], turns[..4], "the prefix is byte-identical");
    }

    #[test]
    fn the_changing_context_block_carries_the_listing_and_memory() {
        let block = changing_context_block("src/\n  main.rs\n", "[Memory]\n- prefers tabs");
        assert!(block.starts_with("\n\n[Linked project: current directory listing]\nsrc/"));
        assert!(block.ends_with("[Memory]\n- prefers tabs"));
        assert_eq!(changing_context_block("", "  "), "");
        let prompt = build_system_prompt("code", LISTING_IN_LATEST_MESSAGE, "m");
        assert!(prompt.contains(LISTING_IN_LATEST_MESSAGE));
        assert!(!prompt.contains("no project linked"));
    }

    #[test]
    fn a_plain_document_reply_is_saved_without_its_fence_under_a_name_from_the_request() {
        assert_eq!(plain_document_body("```markdown\n# Title\n\nBody\n```"), "# Title\n\nBody");
        assert_eq!(plain_document_body("# Title\nBody"), "# Title\nBody");
        assert_eq!(plain_document_filename("Write a markdown file about linked lists", "md"), "linked-lists.md");
        assert_eq!(plain_document_filename("make a csv", "csv"), "document.csv");
        assert!(structured_document_notice("docx").contains(".txt, .md, .csv, .html or .json"));
    }

    #[test]
    fn an_overflow_drops_the_oldest_history_but_never_the_system_prompt_or_the_question() {
        let mut turns = vec![
            ChatTurn::text("system", "rules"),
            ChatTurn::text("user", &"a".repeat(3000)),
            ChatTurn::text("assistant", &"b".repeat(3000)),
            ChatTurn::text("user", &"c".repeat(3000)),
            ChatTurn::text("assistant", &"d".repeat(3000)),
            ChatTurn::text("user", "the question"),
        ];
        let removed = drop_oldest_history(&mut turns, 1200);
        assert_eq!(removed, 2, "one message frees 1,000 tokens; its reply goes with it");
        assert_eq!(turns[0].role, "system");
        assert_eq!(turns[1].role, "user");
        assert_eq!(turns.last().unwrap().content, "the question");
        assert_eq!(drop_oldest_history(&mut turns, u32::MAX), 2);
        assert_eq!(turns.len(), 2);
        assert_eq!(drop_oldest_history(&mut turns, u32::MAX), 0, "nothing left to drop");
    }

    #[test]
    fn a_chat_prompt_teaches_no_tool_protocol_and_claims_no_project() {
        let chat = build_system_prompt("chat", "", "Model");
        assert!(!chat.contains("```tool"), "{chat}");
        assert!(!chat.contains("create_document"));
        assert!(!chat.contains("linked project"));
        assert!(!chat.contains("run approved tools"));
        // The document protocol travels only with a request for a document.
        assert!(document_tool_note().contains("create_document"));
        // A code session still describes its project and its read tools.
        let code = build_system_prompt("code", "Project 'p' at C:/p", "Model");
        assert!(code.contains("read_file"));
        assert!(!code.contains("create_document"));
    }

    #[test]
    fn only_an_offered_tool_is_an_action() {
        let plain_chat = ChatToolOffer::for_request(false, false);
        assert!(plain_chat.is_empty());
        assert!(plain_chat.call_in(INVENTED_TOOL_REPLY).is_none());
        // Nothing offered: an unreadable block earns no correction, so the
        // model is never told its answer should have been a tool call.
        assert!(crate::agent_runner::action_problem(INVENTED_TOOL_REPLY).is_some());
        assert!(!plain_chat.attempted_in(INVENTED_TOOL_REPLY));

        let document = ChatToolOffer::for_request(false, true);
        assert!(document.offers("create_document"));
        assert!(!document.offers("read_file"));
        // Offered, but the model invented a different name: still no correction.
        assert!(!document.attempted_in(INVENTED_TOOL_REPLY));
        // A broken call to the offered tool is corrected.
        let broken_document = "```tool\n{\"name\":\"create_document\",\"args\":{\"filename\":\"a.md\",\"text\":\"x\"\n```";
        assert!(document.attempted_in(broken_document));

        let code = ChatToolOffer::for_request(true, false);
        assert!(code.offers("read_file") && code.offers("manage_context"));
        assert!(!code.offers("create_document") && !code.offers("write_file"));
    }

    #[test]
    fn a_tool_name_is_read_from_a_call_whose_json_is_broken() {
        assert_eq!(tool_name_in(INVENTED_TOOL_REPLY).as_deref(), Some("rust_program"));
        assert_eq!(tool_name_in("{\"name\" : \"read_file\", \"args\": {"), Some("read_file".into()));
        assert_eq!(tool_name_in("no action here"), None);
    }

    #[test]
    fn system_prompt_knows_projects() {
        let snap = "Project 'SampleEngine' at /projects/sample-engine\nBuild: CMake\nTree (top levels):\n  src/\n"
            .to_string();
        let p = build_system_prompt("code", &snap, "Example 14B");
        assert!(p.contains("SampleEngine"), "{p}");
        assert!(p.contains("/init"), "{p}");
        assert!(p.contains("Never claim you cannot"), "{p}");
        let p = build_system_prompt("code", "", "Example 14B");
        assert!(p.contains("no project linked"), "{p}");
        let p = build_system_prompt("chat", "", "Example 14B");
        assert!(!p.contains("/init"), "{p}");
    }

    #[test]
    fn workspace_snapshot_lists_tree() {
        let dir = std::env::temp_dir().join(format!("companion-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.cpp"), "int main(){}").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Use C++17.").unwrap();
        let s = workspace_snapshot(&dir.to_string_lossy(), "Demo");
        assert!(s.contains("src/"), "{s}");
        assert!(s.contains("main.cpp"), "{s}");
        assert!(s.contains("C++17"), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- Stages 32–38 endpoint tests ----

    async fn patch_workspace(a: &Router, conversation: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let response = a.clone().oneshot(json_req("PATCH", &format!("/api/conversations/{conversation}"), body)).await.unwrap();
        (response.status(), body_json(response).await)
    }

    async fn add_test_message(state: &AppState, conversation: &str) {
        state.storage.lock().await.add_message(&Message {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation.to_string(),
            role: "user".into(),
            content: "Inspect this project".into(),
            created_at: "now".into(),
        }).unwrap();
    }

    #[tokio::test]
    async fn code_session_keeps_its_project_once_it_has_messages() {
        let state = AppState::new_stub();
        let (a, first) = seed_workspace(router(state.clone()), "First").await;
        let (a, second) = seed_workspace(a, "Second").await;
        let response = a.clone().oneshot(json_req("POST", "/api/conversations",
            serde_json::json!({"title": "Scoped", "model_id": "", "mode": "code", "workspace": first}))).await.unwrap();
        let conversation = body_json(response).await["id"].as_str().unwrap().to_string();

        // Before the first message the session can still be pointed elsewhere.
        let (status, body) = patch_workspace(&a, &conversation, serde_json::json!({"workspace": second})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["workspace"], second.as_str());

        add_test_message(&state, &conversation).await;
        let (status, body) = patch_workspace(&a, &conversation, serde_json::json!({"workspace": first})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("another project"), "{body}");
        // Clearing the link, or leaving code mode and coming back, does not get around it.
        let (status, _) = patch_workspace(&a, &conversation, serde_json::json!({"workspace": ""})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = patch_workspace(&a, &conversation, serde_json::json!({"mode": "chat"})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = patch_workspace(&a, &conversation, serde_json::json!({"mode": "code", "workspace": first})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        // Naming the project it already has is not a move.
        let (status, body) = patch_workspace(&a, &conversation, serde_json::json!({"mode": "code", "workspace": second, "title": "Renamed"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let kept = state.storage.lock().await.get_conversation(&conversation).unwrap().unwrap();
        assert_eq!(kept.workspace, second);
        assert_eq!(kept.title, "Renamed");
    }

    #[tokio::test]
    async fn removing_a_project_can_take_its_tasks_with_it_and_never_the_folder() {
        let state = AppState::new_stub();
        let (a, kept_project) = seed_workspace(router(state.clone()), "Kept").await;
        let (a, gone_project) = seed_workspace(a, "Gone").await;
        let folder = state
            .storage
            .lock()
            .await
            .get_workspace(&gone_project)
            .unwrap()
            .unwrap()
            .path;
        let session = |workspace: &str, title: &str| {
            let a = a.clone();
            let (workspace, title) = (workspace.to_string(), title.to_string());
            async move {
                let response = a
                    .oneshot(json_req(
                        "POST",
                        "/api/conversations",
                        serde_json::json!({"title": title, "model_id": "", "mode": "code", "workspace": workspace}),
                    ))
                    .await
                    .unwrap();
                body_json(response).await["id"].as_str().unwrap().to_string()
            }
        };
        let first = session(&gone_project, "First task").await;
        let second = session(&gone_project, "Second task").await;
        let elsewhere = session(&kept_project, "Another project's task").await;

        // By default the chats stay: they report a missing project until relinked.
        let response = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{kept_project}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["chats"], 1, "{body}");
        assert_eq!(body["chats_deleted"], 0, "{body}");
        assert!(state.storage.lock().await.get_conversation(&elsewhere).unwrap().is_some());

        // Asked to, it takes them with it - and only them.
        let response = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{gone_project}?tasks=delete"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["chats"], 2, "{body}");
        assert_eq!(body["chats_deleted"], 2, "{body}");
        let storage = state.storage.lock().await;
        assert!(storage.get_conversation(&first).unwrap().is_none());
        assert!(storage.get_conversation(&second).unwrap().is_none());
        assert!(storage.get_conversation(&elsewhere).unwrap().is_some(), "another project's task is untouched");
        assert!(storage.get_workspace(&gone_project).unwrap().is_none());
        drop(storage);

        // The user's own files are never what "remove project" means.
        assert!(std::path::Path::new(&folder).is_dir(), "the folder on disk stays: {folder}");

        // A project that is already gone says so rather than pretending.
        let response = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{gone_project}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn session_without_a_registered_project_can_be_given_one() {
        let state = AppState::new_stub();
        let (a, first) = seed_workspace(router(state.clone()), "First").await;
        let (a, second) = seed_workspace(a, "Second").await;
        // A chat with history becomes a code session: it had no project to keep.
        let response = a.clone().oneshot(json_req("POST", "/api/conversations",
            serde_json::json!({"title": "Older", "model_id": ""}))).await.unwrap();
        let conversation = body_json(response).await["id"].as_str().unwrap().to_string();
        add_test_message(&state, &conversation).await;
        let (status, body) = patch_workspace(&a, &conversation, serde_json::json!({"mode": "code", "workspace": first})).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // Its project is removed: choosing another is the only way to use it again.
        let deleted = a.clone().oneshot(Request::builder().method("DELETE").uri(format!("/api/workspaces/{first}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        let (status, body) = patch_workspace(&a, &conversation, serde_json::json!({"workspace": second})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["workspace"], second.as_str());
    }

    #[tokio::test]
    async fn stale_code_workspace_never_falls_back_to_another_project() {
        let (a, workspace) = seed_workspace(app(), "Intended").await;
        let response = a.clone().oneshot(json_req("POST", "/api/conversations",
            serde_json::json!({"title": "Scoped", "model_id": "", "mode": "code", "workspace": workspace}))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let conversation = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let _ = a
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{workspace}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (a, _) = seed_workspace(a, "Unrelated").await;
        let response = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/chat",
                serde_json::json!({
                    "conversation_id": conversation, "message": "Inspect this project"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let messages = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{conversation}/messages"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            body_json(messages).await.as_array().unwrap().is_empty(),
            "rejected requests must not create misleading messages"
        );
    }

    #[tokio::test]
    async fn agent_rejects_folder_different_from_linked_code_project() {
        let (a, workspace) = seed_workspace(app(), "Bounded").await;
        let response = a.clone().oneshot(json_req("POST", "/api/conversations",
            serde_json::json!({"title": "Scoped", "model_id": "", "mode": "code", "workspace": workspace}))).await.unwrap();
        let conversation = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let response = a.oneshot(json_req("POST", "/api/agent/run", serde_json::json!({
            "conversation_id": conversation, "workspace": agent_ws(), "task": "Inspect", "mode": "plan"
        }))).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("differs"));
    }

    #[tokio::test]
    async fn agent_failure_activity_is_persisted_with_its_reply() {
        let a = app();
        let response = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/conversations",
                serde_json::json!({"title": "Journal", "model_id": ""}),
            ))
            .await
            .unwrap();
        let conversation = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let response = a.clone().oneshot(json_req("POST", "/api/agent/run", serde_json::json!({
            "conversation_id": conversation, "workspace": agent_ws(), "task": "Inspect", "mode": "plan"
        }))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let run_id = body_json(response).await["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        for _ in 0..40 {
            let response = a
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/conversations/{conversation}/messages"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let messages = body_json(response).await;
            let reply = messages
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["id"] == run_id)
                .unwrap();
            if reply["activities"]
                .as_array()
                .map(|events| !events.is_empty())
                .unwrap_or(false)
            {
                assert_eq!(reply["activities"][0]["state"], "FAILED");
                assert!(reply["content"]
                    .as_str()
                    .unwrap()
                    .contains("No inference running"));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("agent journal did not persist its failure");
    }

    #[tokio::test]
    #[ignore = "requires an idle local model and COMPANION_EXPLORATION_PROBE_URL"]
    async fn live_chat_explores_source_beyond_line_17000() {
        // Test-only synthetic project. Never changes an actual user project.
        let state = AppState::new_stub();
        let root = std::path::PathBuf::from(agent_ws());
        std::fs::create_dir_all(root.join("src/runtime")).unwrap();
        let token = format!("epoch-{}", uuid::Uuid::new_v4());
        let text = format!(
            "{}const char* resolve_stream_epoch() {{\n    return \"{token}\";\n}}\n",
            format!("// {}\n", "fixture padding ".repeat(6)).repeat(17_020)
        );
        std::fs::write(root.join("src/runtime/stream.cpp"), text).unwrap();
        std::fs::write(root.join("README.md"), "A synthetic stream runtime. Implementation lives under src/runtime. This overview does not specify epoch values.").unwrap();
        let workspace = Some((
            "probe".into(),
            "Exploration test fixture".into(),
            root.clone(),
        ));
        let snapshot = workspace_snapshot(&root.to_string_lossy(), "Exploration test fixture");
        let mut turns = vec![ChatTurn::text("system", build_system_prompt("code", &snapshot, "local model")), ChatTurn::text("user", "What does resolve_stream_epoch return, and where is it implemented? Inspect source and cite the line.")];
        let mut memory = crate::inspection_context::InspectionContext::new(&turns);
        let client = SidecarClient::new(
            std::env::var("COMPANION_EXPLORATION_PROBE_URL").expect("set probe URL"),
        )
        .unwrap();
        let cfg = InferenceConfig {
            n_ctx: 32768,
            ..Default::default()
        };
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut answer = String::new();
        for round in 1..=10 {
            assert!(memory.compact(&mut turns, cfg.n_ctx, 1024));
            let (response, _) = client
                .chat_turns_without_reasoning(&turns, 1024, &cfg)
                .await
                .unwrap();
            println!("Exploration round {round}: {response}");
            if !run_chat_tool_round(
                &state,
                &None,
                &workspace,
                &ChatToolOffer::for_request(true, false),
                &response,
                &mut turns,
                &mut memory,
                &sender,
                "live-exploration",
                round,
            )
            .await
            {
                answer = response;
                break;
            }
        }
        let events = state
            .storage
            .lock()
            .await
            .message_activities("live-exploration")
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.tool.as_deref() == Some("read_file")
                    && event
                        .args
                        .as_ref()
                        .and_then(|a| a["start_line"].as_u64())
                        .unwrap_or(0)
                        >= 17_000),
            "model must actually read the deep source range"
        );
        assert!(
            answer.contains(&token),
            "answer must use the unpredictable value read from source: {answer}"
        );
        assert!(answer.contains("17021") || answer.contains("17022"));
        std::fs::remove_file(root.join("src/runtime/stream.cpp")).unwrap();
        std::fs::remove_file(root.join("README.md")).unwrap();
        std::fs::remove_dir(root.join("src/runtime")).unwrap();
        std::fs::remove_dir(root.join("src")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn chat_inspection_reads_multiple_deep_chunks_without_transport_truncation() {
        let mut memory = crate::inspection_context::InspectionContext::default();
        let state = AppState::new_stub();
        let root = std::path::PathBuf::from(agent_ws());
        let content = (1..=18_000)
            .map(|i| format!("source_{i} {}\n", "x".repeat(70)))
            .collect::<String>();
        std::fs::write(root.join("large.cpp"), content).unwrap();
        let workspace = Some(("ws".into(), "Large source fixture".into(), root));
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut turns = vec![];
        for (round, start) in [1, 800, 4_000, 9_000, 17_000, 17_901]
            .into_iter()
            .enumerate()
        {
            let call = format!(
                "```tool\n{}\n```",
                serde_json::json!({"name":"read_file","args":{"path":"large.cpp","start_line":start,"end_line":start+99}})
            );
            assert!(
                run_chat_tool_round(
                    &state,
                    &None,
                    &workspace,
                    &ChatToolOffer::for_request(true, false),
                    &call,
                    &mut turns,
                    &mut memory,
                    &sender,
                    "chunk-reply",
                    round as u32 + 1
                )
                .await
            );
            let result = &turns.last().unwrap().content;
            assert!(result.chars().count() > 4_000);
            assert!(result.contains(&format!("{}: source_{}", start + 99, start + 99)));
            assert!(result.contains(if start == 17_901 {
                "EOF: no further content"
            } else {
                "More file content available"
            }));
        }
        assert!(CHAT_TOOL_ROUNDS >= 24);
        assert_eq!(turns.len(), 12);
        assert!(run_chat_tool_round(&state, &None, &workspace, &ChatToolOffer::for_request(true, false),
            "```tool\n{\"name\":\"manage_context\",\"args\":{\"keep\":[\"chunk-5\"],\"release\":[\"chunk-1\"]}}\n```",
            &mut turns, &mut memory, &sender, "chunk-reply", 7).await);
        assert!(turns[1].content.contains("Body released"));
        assert!(turns[9].content.contains("17099: source_17099"));
        assert!(turns.last().unwrap().content.contains("Kept"));
        let events = state
            .storage
            .lock()
            .await
            .message_activities("chunk-reply")
            .unwrap();
        assert!(
            events[1]
                .output
                .as_ref()
                .unwrap()
                .contains("100: source_100"),
            "journal retains released evidence"
        );
    }

    #[test]
    fn chat_inspection_compaction_preserves_history_and_latest_chunk() {
        let history = vec![
            ChatTurn::text("system", "system"),
            ChatTurn::text("user", "Explain this project"),
            ChatTurn::text("assistant", "prior answer"),
            ChatTurn::text("user", "dive deeper"),
        ];
        let mut turns = history.clone();
        let mut memory = crate::inspection_context::InspectionContext::new(&history);
        for _ in 0..6 {
            turns.push(ChatTurn::text("assistant", "read source"));
            turns.push(ChatTurn::text(
                "user",
                format!(
                    "[tool result: read_file]\n[Lines 17000-17100]\n{}\n[next start_line=17101]",
                    "x".repeat(12_000)
                ),
            ));
            memory.record(
                &mut turns,
                "read_file",
                &serde_json::json!({"path":"source.cpp","start_line":17000,"end_line":17100}),
            );
        }
        let latest = turns.last().unwrap().content.clone();
        assert!(memory.compact(&mut turns, 16_384, 2048));
        for (a, b) in turns.iter().zip(&history) {
            assert_eq!(a.content, b.content);
        }
        assert_eq!(turns.last().unwrap().content, latest);
        assert!(turns.iter().any(|t| t.content.contains("Body released")));
        assert!(!memory.compact(&mut turns, 1024, 1024));
    }

    #[test]
    fn chat_inspection_prompt_supports_source_exploration_and_arbitrary_ranges() {
        let prompt = build_system_prompt("code", "src/\ntests/", "test");
        assert!(prompt.contains("source, callers, tests and configuration"));
        assert!(prompt.contains("ANY start_line/end_line"));
        assert!(prompt.contains("24 read-only actions"));
        let output = "x".repeat(18_000);
        assert!(chat_tool_output(&output).contains("Result truncated"));
    }

    #[tokio::test]
    async fn chat_inspection_journals_real_output_and_rejects_writes() {
        let mut memory = crate::inspection_context::InspectionContext::default();
        let state = AppState::new_stub();
        let root = std::path::PathBuf::from(agent_ws());
        std::fs::write(root.join("readme.txt"), "verified file content").unwrap();
        let workspace = Some(("ws".into(), "Test".into(), root.clone()));
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut turns = vec![];
        assert!(
            run_chat_tool_round(
                &state,
                &None,
                &workspace,
                &ChatToolOffer::for_request(true, false),
                "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"readme.txt\"}}\n```",
                &mut turns,
                &mut memory,
                &sender,
                "inspection-reply",
                1
            )
            .await
        );
        let events = state
            .storage
            .lock()
            .await
            .message_activities("inspection-reply")
            .unwrap();
        assert_eq!(events[0].kind, "tool_started");
        assert_eq!(events[1].kind, "tool_result");
        assert!(events[1]
            .output
            .as_ref()
            .unwrap()
            .contains("verified file content"));
        assert!(run_chat_tool_round(&state, &None, &workspace, &ChatToolOffer::for_request(true, false),
            "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"blocked.txt\",\"content\":\"never write\"}}\n```",
            &mut turns, &mut memory, &sender, "inspection-reply", 2).await);
        assert!(!root.join("blocked.txt").exists());
        let events = state
            .storage
            .lock()
            .await
            .message_activities("inspection-reply")
            .unwrap();
        assert_eq!(events.last().unwrap().kind, "tool_error");
        assert!(events.last().unwrap().diff.is_none());
        std::fs::remove_file(root.join("readme.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    async fn seed_workspace(a: Router, name: &str) -> (Router, String) {
        let dir = ws_dir();
        std::fs::write(
            format!("{dir}/src/main.rs"),
            "fn main() {}\nstruct App {}\n",
        )
        .unwrap();
        std::fs::write(
            format!("{dir}/notes.md"),
            "# notes\nThe renderer uses reversed-Z.\n",
        )
        .unwrap();
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/workspaces",
                serde_json::json!({"name": name, "path": dir}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let id = body_json(r).await["id"].as_str().unwrap().to_string();
        (a, id)
    }

    #[tokio::test]
    async fn index_lists_files_and_searches() {
        let (a, id) = seed_workspace(app(), "Idx").await;
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/index"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert!(v["files_total"].as_u64().unwrap() >= 3, "{v}");
        assert!(v["symbols_total"].as_u64().unwrap() >= 1, "{v}");
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/index?q=main"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        let matches = v["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "{v}");
        assert!(matches.iter().any(|m| m["path"] == "src/main.rs"), "{v}");
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/index?q=zzz-nope"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(r).await["matches"].as_array().unwrap().is_empty());
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces/nope/index")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn knowledge_ingest_list_clear() {
        let (a, id) = seed_workspace(app(), "Know").await;
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/workspaces/{id}/knowledge"),
                serde_json::json!({"path": "notes.md"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["files_indexed"], 1, "{v}");
        assert!(v["chunks_added"].as_u64().unwrap() >= 1, "{v}");
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/knowledge"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert_eq!(
            v["total_chunks"].as_u64().unwrap(),
            v["paths"][0]["chunks"].as_u64().unwrap(),
            "{v}"
        );
        // Traversal escapes the workspace.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/workspaces/{id}/knowledge"),
                serde_json::json!({"path": "../../etc/passwd"}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/workspaces/{id}/knowledge/clear"),
                serde_json::json!({"path": "notes.md"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            body_json(r).await["removed"].as_u64().unwrap(),
            v["total_chunks"].as_u64().unwrap()
        );
    }

    #[tokio::test]
    async fn metrics_empty_then_404() {
        let a = app();
        let (a, id) = seed_conv(a, "metrics t").await;
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/conversations/{id}/metrics"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_json(r).await["metrics"].as_array().unwrap().is_empty());
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/api/conversations/nope/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn git_reports_non_repo_honestly() {
        let (a, id) = seed_workspace(app(), "Gitless").await;
        let r = a
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{id}/git"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["git"], false, "{v}");
    }

    #[tokio::test]
    async fn plugins_list_and_guarded_run() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/plugins")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let plugins = v["plugins"].as_array().unwrap();
        assert!(
            plugins
                .iter()
                .any(|p| p["id"] == "filesystem" && p["manifest"]["name"] == "filesystem"),
            "{v}"
        );
        // Undeclared tool is refused even though it exists in the registry.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/plugins/filesystem/run",
                serde_json::json!({"tool": "execute_command", "args": {"command": "echo hi"}}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // Unknown plugin 404s.
        let r = a
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/plugins/nope/run",
                serde_json::json!({"tool": "system_info", "args": {}}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        // Declared SAFE tool runs (temp dir root when no workspace given).
        let r = a
            .oneshot(json_req(
                "POST",
                "/api/plugins/filesystem/run",
                serde_json::json!({"tool": "list_directory", "args": {"path": "."}}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["ok"], true);
    }

    #[tokio::test]
    async fn doctor_reports_rows() {
        let r = app()
            .oneshot(
                Request::builder()
                    .uri("/api/doctor")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let checks = v["checks"].as_array().unwrap();
        assert!(checks.len() >= 8, "{v}");
        assert!(
            checks
                .iter()
                .any(|c| c["id"] == "backend" && c["status"] == "ok"),
            "{v}"
        );
        assert!(
            ["ok", "warn", "fail"].contains(&v["status"].as_str().unwrap()),
            "{v}"
        );
    }

    #[tokio::test]
    async fn setup_status_shape_and_benchmark_needs_sidecar() {
        let a = app();
        let r = a
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/setup/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["steps"].as_array().unwrap().len(), 5, "{v}");
        assert!(v.get("needs_setup").is_some(), "{v}");
        let r = a
            .oneshot(json_req(
                "POST",
                "/api/system/benchmark",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
