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
use crate::permissions::{AutonomyLevel, PermissionManager};
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

const MAX_MESSAGE_CHARS: usize = 200_000;
const MAX_TITLE_CHARS: usize = 200;
const CHAT_TOOL_ROUNDS: u32 = 24;
/// Stage 6: history window fed to the model (§21). Older turns are dropped,
/// newest kept; the context endpoint reports exactly what was dropped.
const HISTORY_TURNS: usize = 20;
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
    pub attachments_dir: PathBuf,
    /// Stage 21: staged model-load progress (validating → loading → ready).
    pub load_progress: Arc<tokio::sync::RwLock<LoadProgress>>,
    /// Stage 32: repo-index cache per workspace id: (dir mtime, index).
    pub repo_index: Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, (u64, crate::repo_index::RepoIndex)>>,
    >,
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
            .unwrap_or_default();
        // Only the explicit global preference is durable. Temporary grants and
        // one-time approvals belong to the old process and are never restored.
        let permissions = PermissionManager::new(if settings.agent.autonomous_enabled {
            AutonomyLevel::Autonomous
        } else {
            AutonomyLevel::Assisted
        });
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
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("http client"),
            permissions: Arc::new(tokio::sync::RwLock::new(permissions)),
            settings: Arc::new(tokio::sync::RwLock::new(settings)),
            settings_update: Arc::new(tokio::sync::Mutex::new(())),
            runtime_update: Arc::new(tokio::sync::Mutex::new(())),
            storage: Arc::new(tokio::sync::Mutex::new(storage)),
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
        }
    }
}

/// Consistent error envelope (§52): human message + actionable hint.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    hint: String,
}

#[derive(Debug)]
struct ApiError {
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
        .route("/api/chat/classify", post(classify_request))
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
    let (found, warnings) = crate::models::scan_models_dir(&s.models_dir);
    if !found.is_empty() {
        let mut models = s.models.write().await;
        for model in found {
            if let Err(error) = models.register(model) {
                tracing::warn!("skipping discovered model: {error}");
            }
        }
    }
    for warning in warnings {
        tracing::warn!("{warning}");
    }
    Json(s.models.read().await.list())
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
        Ok(id) => set_progress(&s, id, "ready", "Model loaded and ready for inference.").await,
        Err(e) => set_progress(&s, req.id.trim(), "error", &e.message).await,
    }
    out.map(|id| Json(serde_json::json!({"loaded": id})))
}

async fn unload_models(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let _update = s.runtime_update.lock().await;
    guard_agent_running(&s, false).await?;
    s.models.write().await.unload_all();
    s.inference.write().await.unload();
    s.llama.write().await.stop().await;
    tracing::info!("models unloaded");
    Ok(Json(serde_json::json!({"unloaded": true})))
}

/// Stage 3: inference engine status for diagnostics (§80).
async fn inference_status(State(s): State<AppState>) -> Json<InferenceStatus> {
    let mut llama = s.llama.write().await;
    let running = llama.is_running();
    let base_url = llama.base_url();
    let last_error = llama.last_error.clone();
    let binary_found = SidecarBinary::detect(&s.models_dir).is_ok();
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
            matches!(
                r.state,
                crate::agent::AgentState::Planning
                    | crate::agent::AgentState::ExecutingTool
                    | crate::agent::AgentState::WaitingPermission
                    | crate::agent::AgentState::Observing
            )
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
    let projector_path = model
        .as_ref()
        .and_then(|m| m.projector_file.as_ref().map(|p| m.dir.join(p)))
        .filter(|p| p.is_file());
    let mut cfg = InferenceConfig {
        model_path: gguf.clone(),
        projector_path,
        n_ctx: n_ctx.unwrap_or(settings.inference.context_size),
        n_batch: settings.inference.batch_size,
        n_threads: n_threads.unwrap_or(settings.hardware.cpu_threads),
        n_gpu_layers: n_gpu_layers.unwrap_or(settings.hardware.gpu_layers),
        flash_attn: settings.hardware.flash_attention,
        kv_cache_gpu: settings.hardware.kv_cache_gpu,
        temperature: settings.inference.temperature,
        top_p: settings.inference.top_p,
        top_k: settings.inference.top_k,
        repeat_penalty: settings.inference.repeat_penalty,
        seed: None,
        ..InferenceConfig::default()
    };
    let policy =
        crate::inference::resolve_runtime_policy(model.as_ref(), &cfg, settings.runtime_auto);
    policy.apply_to(&mut cfg);

    let binary = match SidecarBinary::detect(&s.models_dir) {
        Ok(b) => b,
        Err(e) => {
            s.llama.write().await.last_error = Some(e.to_string());
            return Err(ApiError::not_found(e.to_string()));
        }
    };

    // Stop any previous sidecar before starting (§15 switch semantics).
    s.llama.write().await.stop().await;
    s.models.write().await.unload_all();
    s.inference.write().await.unload();
    let devices = if settings.runtime_auto {
        binary.devices().await
    } else {
        crate::runtime_selection::Devices::Unknown
    };
    match crate::runtime_selection::load_with_fallback(
        cfg,
        settings.runtime_auto,
        devices,
        |attempt| RunningSidecar::spawn(&binary.0, attempt, port),
    )
    .await
    {
        Ok((sidecar, notice)) => {
            let base_url = sidecar.base_url.clone();
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
                serde_json::json!({"started": true, "base_url": base_url, "model": model_id, "notice": notice, "runtime_policy": cfg.runtime_policy}),
            )
        }
        Err(e) => {
            let msg = e.to_string();
            s.llama.write().await.last_error = Some(msg.clone());
            Err(ApiError::internal(format!(
                "could not start inference: {msg}"
            )))
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
    Ok(Json(serde_json::json!({"stopped": true})))
}

/// Re-scan the models directory (§9–§10). Registers new valid entries.
async fn scan_models(State(s): State<AppState>) -> Json<serde_json::Value> {
    let dir = s.models_dir.clone();
    let (found, warnings) = crate::models::scan_models_dir(&dir);
    let mut registered = 0usize;
    {
        let mut mm = s.models.write().await;
        for m in found {
            if mm.register(m).is_ok() {
                registered += 1;
            }
        }
    }
    for w in &warnings {
        tracing::warn!("{w}");
    }
    Json(serde_json::json!({"registered": registered, "warnings": warnings}))
}

/// Usable VRAM: measured via nvidia-smi when present, else the (stubbed)
/// sysinfo enumeration. Centralized so estimates never assume infinite RAM
/// on machines whose GPU the stub cannot see (§160).
fn measured_vram_gb(hw: &hardware::HardwareReport) -> f64 {
    crate::metrics::vram_info()
        .map(|(_, total)| total)
        .unwrap_or_else(|| hw.gpus.iter().map(|g| g.vram_gb).fold(0.0, f64::max))
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
