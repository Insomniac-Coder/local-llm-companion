//! llama.cpp integration via `llama-server` sidecar (Stage 3).
//!
//! Why a sidecar instead of native bindings?
//! - This machine has no C++ toolchain (no g++/cmake), so compiling
//!   llama.cpp inline is not possible here. A sidecar needs no build.
//! - Isolation: a GGML assert / OOM kills the sidecar, not the backend,
//!   so conversation history and the API stay alive (§82–83).
//! - The rest of the app still talks to `IInferenceEngine` (§100); the
//!   sidecar client is one more engine behind that boundary.
//!
//! `llama-server` speaks an OpenAI-compatible HTTP API on 127.0.0.1:
//! `GET /health`, `POST /v1/chat/completions`, `POST /tokenize`.

use crate::inference::{
    thiserror_stub::{InferenceError, SidecarFailure},
    ChatTemplateShape, InferenceConfig, Metrics, OutputTiming,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::{Child, Command};

pub const DEFAULT_SIDECAR_PORT: u16 = 3888;
/// A cold 9 GB model on a laptop drive can take several minutes to page in;
/// a dead worker still fails immediately because the child exit is checked
/// on every poll.
const HEALTH_TIMEOUT_SECS: u64 = 300;

/// Where the `llama-server` binary lives.
#[derive(Debug, Clone)]
pub struct SidecarBinary(pub PathBuf);

impl SidecarBinary {
    /// Query the runtime that will actually execute the model. In particular,
    /// lack of nvidia-smi never disables a Vulkan/SYCL integrated GPU.
    pub async fn devices(&self) -> crate::runtime_selection::Devices {
        let mut command = Command::new(&self.0);
        command
            .arg("--list-devices")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let Ok(mut child) = command.spawn() else {
            return crate::runtime_selection::Devices::Unknown;
        };
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            tasks.push(drain_worker_log(pipe, stdout.clone()));
        }
        if let Some(pipe) = child.stderr.take() {
            tasks.push(drain_worker_log(pipe, stderr.clone()));
        }
        let success = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(status)) => status.success(),
            _ => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                false
            }
        };
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_millis(500), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        let out = stdout
            .lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let err = stderr
            .lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        crate::runtime_selection::parse_devices(&format!("{err}\n{out}"), success)
    }

    /// Whether this runtime's `--help` lists `argument`. An older runtime given
    /// through the override refuses arguments it does not know, and "unknown
    /// argument" is never retried, so an optional argument is only passed when
    /// the runtime lists it.
    pub async fn supports_argument(&self, argument: &str) -> bool {
        self.help_text()
            .await
            .map(|text| text.split(|c: char| c.is_whitespace() || c == ',').any(|word| word == argument))
            .unwrap_or(false)
    }

    /// The built-in chat formats this runtime's `--help` lists.
    pub async fn builtin_chat_templates(&self) -> Vec<String> {
        self.help_text().await.map(|text| listed_builtin_chat_templates(&text)).unwrap_or_default()
    }

    /// This runtime's `--help` text, read once per binary and modification
    /// time. None when it cannot be read; that is not remembered.
    async fn help_text(&self) -> Option<Arc<String>> {
        use std::collections::HashMap;
        use std::sync::OnceLock;
        type Key = (PathBuf, Option<std::time::SystemTime>);
        static SEEN: OnceLock<Mutex<HashMap<Key, Arc<String>>>> = OnceLock::new();
        let modified = std::fs::metadata(&self.0).and_then(|meta| meta.modified()).ok();
        let key = (self.0.clone(), modified);
        if let Some(known) = SEEN.get_or_init(Default::default).lock().ok().and_then(|seen| seen.get(&key).cloned()) {
            return Some(known);
        }
        let mut command = Command::new(&self.0);
        command
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let child = command.spawn().ok()?;
        let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await.ok()?.ok()?;
        let text = Arc::new(format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
        if let Ok(mut seen) = SEEN.get_or_init(Default::default).lock() {
            seen.insert(key, text.clone());
        }
        Some(text)
    }
    /// Where the llama-server runtime is, in order of authority (the error
    /// tells the user how to build it, §52):
    /// 1. COMPANION_LLAMA_SERVER_BIN, an explicit override;
    /// 2. `runtime_dir` (runtime/bin at the installation root), built from the
    ///    pinned commit by scripts/build-runtime.ps1 or build-runtime.sh;
    /// 3. PATH, last. It used to be searched first, so any llama.cpp installed
    ///    system-wide silently replaced the project's pinned build.
    pub fn detect(runtime_dir: &Path) -> Result<Self, InferenceError> {
        // Empty counts as unset, as for the other COMPANION_* folders (a shell
        // can export an empty variable; run.sh then builds runtime/bin).
        if let Some(p) = std::env::var("COMPANION_LLAMA_SERVER_BIN").ok().filter(|p| !p.is_empty()) {
            let pb = PathBuf::from(&p);
            if pb.is_file() {
                return Ok(Self(pb));
            }
            return Err(InferenceError::Generation(format!(
                "COMPANION_LLAMA_SERVER_BIN points at '{p}' but no file is there. \
                 Unset it to use the project runtime in runtime/bin."
            )));
        }
        let exe = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        let bundled = runtime_dir.join(exe);
        if bundled.is_file() {
            return Ok(Self(bundled));
        }
        if let Some(p) = search_path(exe) {
            return Ok(Self(p));
        }
        let script = if cfg!(windows) {
            "scripts\\build-runtime.ps1"
        } else {
            "scripts/build-runtime.sh"
        };
        Err(InferenceError::Generation(format!(
            "The llama.cpp runtime is not built yet. Run {script} from the project folder \
             (it builds llama-server with the backends this machine supports \
             into runtime/bin), or set COMPANION_LLAMA_SERVER_BIN."
        )))
    }
}

/// The project's runtime folder: runtime/bin at the installation root. It used
/// to be derived from the models folder's parent, so a models folder elsewhere
/// (COMPANION_MODELS_DIR) made a built runtime look missing.
pub fn runtime_dir(install_root: &Path) -> PathBuf {
    install_root.join("runtime").join("bin")
}

fn search_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(exe);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// The names under "list of built-in templates:" in `--help`, which runs over
/// the following lines until the option's `(env: ...)` line.
fn listed_builtin_chat_templates(help: &str) -> Vec<String> {
    let Some(start) = help.find("list of built-in templates:") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for line in help[start + "list of built-in templates:".len()..].lines() {
        let line = line.trim();
        if line.starts_with("(env:") || line.starts_with('-') {
            break;
        }
        names.extend(line.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string));
    }
    names
}

/// Build the `llama-server` argv from our config (§13 advanced panel).
/// Pure function — unit-tested without any binary present.
pub fn server_args(cfg: &InferenceConfig, port: u16) -> Vec<String> {
    let mut a = vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--parallel".into(),
        "1".into(),
        "-m".into(),
        cfg.model_path.display().to_string(),
        "--ctx-size".into(),
        cfg.n_ctx.to_string(),
        "--n-gpu-layers".into(),
        if cfg.n_gpu_layers < 0 {
            "auto".into()
        } else {
            cfg.n_gpu_layers.to_string()
        },
        "--cache-type-k".into(),
        cfg.kv_cache_type_k.clone(),
        "--cache-type-v".into(),
        cfg.kv_cache_type_v.clone(),
        "--flash-attn".into(),
        if cfg.flash_attn_auto {
            "auto"
        } else if cfg.flash_attn {
            "on"
        } else {
            "off"
        }
        .into(),
    ];
    // Zero leaves the runtime's own default (2048 logical / 512 physical);
    // forcing a smaller logical batch only adds decode calls during prefill.
    // llama.cpp caps the micro-batch at the logical batch, so a chosen
    // micro-batch raises the logical batch when that would be smaller.
    let micro_batch = cfg.micro_batch.filter(|size| *size > 0);
    let batch = match (cfg.n_batch, micro_batch) {
        (0, Some(size)) if size > crate::runtime_fit::RUNTIME_DEFAULT_BATCH => size,
        (0, _) => 0,
        (batch, Some(size)) => batch.max(size),
        (batch, None) => batch,
    };
    if batch > 0 {
        a.push("--batch-size".into());
        a.push(batch.to_string());
    }
    if let Some(size) = micro_batch {
        a.push("--ubatch-size".into());
        a.push(size.to_string());
    }
    // The pinned runtime's spelling; `--no-mmap` is deprecated there.
    if cfg.load_without_mmap {
        a.push("--load-mode".into());
        a.push("none".into());
    }
    if cfg.cache_reuse > 0 {
        a.push("--cache-reuse".into());
        a.push(cfg.cache_reuse.to_string());
    }
    if !cfg.speculative.is_empty() && cfg.speculative != "none" {
        a.push("--spec-type".into());
        a.push(cfg.speculative.clone());
        if let Some(tokens) = cfg.spec_draft_n_max {
            a.push("--spec-draft-n-max".into());
            a.push(tokens.to_string());
        }
        if let Some(length) = cfg.spec_ngram_length.filter(|_| cfg.speculative.contains("ngram-simple")) {
            // N-gram drafting drafts nothing when the length is shorter than
            // its lookup (default 12), so a short length shortens the lookup.
            a.push("--spec-ngram-simple-size-n".into());
            a.push(length.clamp(1, 12).to_string());
            a.push("--spec-ngram-simple-size-m".into());
            a.push(length.max(1).to_string());
        }
    }
    // A model file without a chat template: llama.cpp's own formatter for the
    // architecture. Built-in formats exist only outside the Jinja engine, whose
    // one request feature lost is native tool definitions, which the app never
    // sends. ChatML is already the runtime's fallback.
    if let Some(format) = cfg.builtin_chat_format.as_deref().filter(|format| *format != "chatml") {
        a.push("--no-jinja".into());
        a.push("--chat-template".into());
        a.push(format.to_string());
    }
    if cfg.n_threads > 0 {
        a.push("--threads".into());
        a.push(cfg.n_threads.to_string());
    }
    if cfg.n_threads_batch > 0 {
        a.push("--threads-batch".into());
        a.push(cfg.n_threads_batch.to_string());
    }
    if let Some(poll) = cfg.poll {
        a.push("--poll".into());
        a.push(poll.min(100).to_string());
    }
    if let Some(priority) = cfg.priority {
        a.push("--prio".into());
        a.push(priority.clamp(-1, 3).to_string());
    }
    if let Some(mib) = cfg.fit_target_mib {
        a.push("--fit-target".into());
        a.push(mib.to_string());
    }
    if let Some(mib) = cfg.cache_ram_mib {
        a.push("--cache-ram".into());
        a.push(mib.to_string());
    }
    if let Some(mmproj) = &cfg.projector_path {
        a.push("--mmproj".into());
        a.push(mmproj.display().to_string());
    }
    if !cfg.kv_cache_gpu {
        a.push("--no-kv-offload".into());
    }
    if cfg.n_gpu_layers == 0 {
        a.extend(["--device".into(), "none".into(), "--no-op-offload".into()]);
        if cfg.projector_path.is_some() {
            a.push("--no-mmproj-offload".into());
        }
    }
    a
}

/// Separates a load failure's plain message from the runtime log that follows.
pub const RUNTIME_DIAGNOSTIC_MARKER: &str = " Runtime diagnostic: ";

/// A load failure without the runtime log appended to it.
pub fn without_runtime_diagnostic(message: &str) -> &str {
    message.split(RUNTIME_DIAGNOSTIC_MARKER).next().unwrap_or(message)
}

/// The last error-level line of a llama.cpp log (`<time> E <message>`),
/// without its timestamp.
fn last_runtime_error(log: &str) -> Option<String> {
    log.lines()
        .rev()
        .find_map(|line| line.split_once(" E ").map(|(_, message)| message.trim().to_string()))
        .filter(|message| !message.is_empty())
}

/// The runtime's own words for why a model would not load, restated as the
/// cause and what to do. Before this, the only explanation was a log excerpt
/// such as "key not found in model: gemma3.attention.layer_norm_rms_epsilon",
/// which names a symptom of the file's packaging, not the packaging.
/// Recognises llama.cpp's loader messages, not model names.
fn explain_load_failure(log: &str) -> Option<String> {
    let lower = log.to_ascii_lowercase();
    let registry_note = "Files taken from Ollama's registry are packaged for Ollama, whose runtime repairs them while loading; download the upstream GGUF (for example with models/modeldownloader.py hf <owner/repo> <file>).";
    if lower.contains("wrong number of tensors") {
        Some(format!(
            "The model file contains tensors this runtime does not use, typically a vision or audio tower packed in with the language model instead of a separate projector file. {registry_note}"
        ))
    } else if lower.contains("key not found in model") {
        Some(format!(
            "The model file is missing metadata this runtime requires for its architecture. {registry_note}"
        ))
    } else if lower.contains("wrong shape") {
        Some(format!(
            "A tensor in the model file does not have the shape its metadata describes (for example a tokenizer longer than the embedding table). {registry_note}"
        ))
    } else if lower.contains("unknown model architecture") {
        Some("This runtime build does not support the model's architecture. A newer llama.cpp (runtime/llama.cpp.lock.json, then rebuild the runtime) may.".into())
    } else {
        None
    }
}

/// A running sidecar: child process + base URL + the config it was started with.
pub struct RunningSidecar {
    child: Child,
    log_tasks: Vec<tokio::task::JoinHandle<()>>,
    pub base_url: String,
    pub cfg: InferenceConfig,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl RunningSidecar {
    pub async fn spawn(
        binary: &Path,
        cfg: InferenceConfig,
        port: u16,
    ) -> Result<Self, InferenceError> {
        if !cfg.model_path.is_file() {
            return Err(InferenceError::ModelFileMissing(
                cfg.model_path.display().to_string(),
            ));
        }
        // A hard backend termination on Windows can outlive Rust's
        // `kill_on_drop` cleanup and leave its llama-server child holding the
        // model port. Reclaim only an orphan whose executable is exactly the
        // bundled binary we are about to launch. Unrelated listeners remain
        // untouched and still produce the explicit port-in-use error below.
        if reap_orphaned_sidecars(binary) > 0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        // Never mistake an unrelated listener's /health for this new model.
        let port_check = tokio::net::TcpListener::bind(("127.0.0.1", port)).await
            .map_err(|error| InferenceError::Generation(format!(
                "Cannot start a fresh model runtime: port {port} is already in use or unavailable ({error}). Stop that listener or choose a different model-server port."
            )))?;
        drop(port_check);
        let args = server_args(&cfg, port);
        tracing::info!("spawning {} {}", binary.display(), args.join(" "));
        let mut command = Command::new(binary);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|e| {
            InferenceError::Generation(format!(
                "Could not start llama-server ({}): {e}. Is the binary executable?",
                binary.display()
            ))
        })?;
        // Continuously drain both pipes: startup metadata can otherwise fill a
        // pipe and block the worker before /health becomes ready. Keep only a
        // bounded diagnostic tail; never enable prompt logging.
        let log_tail = Arc::new(Mutex::new(Vec::new()));
        let mut log_tasks = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            log_tasks.push(drain_worker_log(stdout, log_tail.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            log_tasks.push(drain_worker_log(stderr, log_tail.clone()));
        }
        let base_url = format!("http://127.0.0.1:{port}");
        // Wait for /health before returning so callers never race startup.
        if let Err(error) = wait_for_health(
            &mut child,
            &base_url,
            Duration::from_secs(HEALTH_TIMEOUT_SECS),
        )
        .await
        {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            for mut task in log_tasks {
                if tokio::time::timeout(Duration::from_millis(500), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                }
            }
            let tail = log_tail
                .lock()
                .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
                .unwrap_or_default();
            // The cause in plain words first; the runtime's log follows the
            // marker for the log and for the GPU-fallback check, and the API
            // shows only what comes before it.
            let started = match error {
                InferenceError::Generation(message) => message,
                other => other.to_string(),
            };
            let plain = match (explain_load_failure(&tail), last_runtime_error(&tail)) {
                (Some(reason), _) => reason,
                (None, Some(line)) => format!("{started} The runtime's last error: {line}"),
                (None, None) => started,
            };
            return Err(InferenceError::Generation(if tail.is_empty() {
                plain
            } else {
                format!("{plain}{RUNTIME_DIAGNOSTIC_MARKER}{tail}")
            }));
        }
        Ok(Self {
            child,
            log_tasks,
            base_url,
            cfg,
            started_at: chrono::Utc::now(),
        })
    }

    /// Graceful stop: SIGTERM-equivalent, then hard kill after 5 s.
    pub async fn stop(mut self) {
        // `Child::kill` on Windows terminates; on unix it SIGKILLs. Good enough
        // for Stage 3; graceful SIGTERM negotiation lands with log streaming.
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        for task in self.log_tasks {
            task.abort();
        }
        tracing::info!("llama-server stopped ({})", self.base_url);
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

fn reap_orphaned_sidecars(binary: &Path) -> usize {
    let target = normalized_executable_path(binary);
    let mut system = sysinfo::System::new_all();
    system.refresh_processes();
    let orphaned: Vec<_> = system
        .processes()
        .values()
        .filter(|process| {
            process
                .exe()
                .map(normalized_executable_path)
                .is_some_and(|path| path == target)
                && process
                    .parent()
                    .map_or(true, |parent| system.process(parent).is_none())
        })
        .collect();
    let mut stopped = 0;
    for process in orphaned {
        if process.kill() {
            stopped += 1;
            tracing::warn!(pid = %process.pid(), "reclaimed orphaned llama-server process");
        }
    }
    stopped
}

fn normalized_executable_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn drain_worker_log(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    tail: Arc<Mutex<Vec<u8>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut chunk = [0u8; 4096];
        while let Ok(count) = pipe.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            if let Ok(mut bytes) = tail.lock() {
                bytes.extend_from_slice(&chunk[..count]);
                let excess = bytes.len().saturating_sub(8192);
                bytes.drain(..excess);
            }
        }
    })
}

async fn wait_for_health(
    child: &mut Child,
    base_url: &str,
    timeout: Duration,
) -> Result<(), InferenceError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| InferenceError::Generation(format!("http client failed: {e}")))?;
    let deadline = tokio::time::Instant::now() + timeout;
    let url = format!("{base_url}/health");
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(InferenceError::Generation(format!(
                    "llama-server exited before becoming ready ({status})."
                )))
            }
            Err(error) => {
                return Err(InferenceError::Generation(format!(
                    "Could not check llama-server process: {error}"
                )))
            }
            Ok(None) => {}
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(());
                }
                return Err(InferenceError::Generation(
                    "llama-server exited during its health check.".into(),
                ));
            }
            _ => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(InferenceError::Generation(format!(
                        "llama-server did not become healthy at {url} within {}s. \
                         Check that the GGUF path is valid and the port is free.",
                        timeout.as_secs()
                    )));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// One connection pool for every request to the sidecar. A fresh client per
/// request (the previous design) meant a new TCP connection per turn and,
/// worse, a 600 s whole-request timeout that killed any long generation.
/// Here only the connect and idle-read timeouts are bounded: a healthy stream
/// never goes 20 minutes without a byte, while a slow CPU prefill of a large
/// context still completes.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(1200))
            .pool_max_idle_per_host(4)
            .tcp_nodelay(true)
            .build()
            .expect("sidecar http client")
    })
}

/// Thin OpenAI-compatible client over the sidecar.
#[derive(Clone)]
pub struct SidecarClient {
    pub base_url: String,
    inner: reqwest::Client,
    recorder: Option<RequestRecorder>,
}

impl std::fmt::Debug for SidecarClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SidecarClient")
            .field("base_url", &self.base_url)
            .field("recording", &self.recorder.is_some())
            .finish()
    }
}

/// What one model request carried and returned, handed to a recorder after
/// the response (or failure) so the caller can store it.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// The body as sent, image data replaced by a placeholder.
    pub body: serde_json::Value,
    pub output: String,
    pub finish_reason: Option<String>,
    /// completed | early_stopped | cancelled | failed
    pub outcome: &'static str,
    pub failure: Option<String>,
    pub prompt_tokens: u32,
    pub cached_tokens: u32,
    pub generated_tokens: u32,
}

pub type RequestRecorder = Arc<dyn Fn(RecordedRequest) + Send + Sync>;

/// Teach the token estimate this model's real characters per token from a
/// request the runtime measured. Requests with images are skipped: an image
/// is hundreds of tokens and no characters.
fn observe_prompt_size(body: &serde_json::Value, prompt_tokens: u32) {
    let Some(messages) = body.get("messages").and_then(|messages| messages.as_array()) else {
        return;
    };
    let mut chars = 0usize;
    for message in messages {
        match message.get("content") {
            Some(serde_json::Value::String(text)) => chars += text.chars().count(),
            Some(serde_json::Value::Array(parts)) => {
                for part in parts {
                    if part.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                        return;
                    }
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        chars += text.chars().count();
                    }
                }
            }
            _ => {}
        }
    }
    crate::agent::observe_prompt(chars, prompt_tokens);
}

/// A copy of a request body with inline image data replaced: a photo is
/// megabytes of base64 that says nothing about why a reply went wrong.
fn without_image_data(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) if text.starts_with("data:image") => {
            serde_json::Value::String(format!("[image data omitted: {} characters]", text.len()))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(items.iter().map(without_image_data).collect()),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter().map(|(key, item)| (key.clone(), without_image_data(item))).collect(),
        ),
        other => other.clone(),
    }
}

/// Agent-control metadata for one completion. Native reasoning text is never
/// retained here: callers need evidence of exhaustion, not its private content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCompletion {
    pub text: String,
    pub metrics: Metrics,
    pub finish_reason: Option<String>,
    pub reasoning_present: bool,
    pub reasoning_tokens: Option<u32>,
    pub native_tool_calls_present: bool,
    /// The host stopped reading because a complete action had already
    /// arrived; the runtime was released without generating the remainder.
    #[serde(default)]
    pub early_stopped: bool,
}

impl AgentCompletion {
    pub fn is_truncated(&self) -> bool {
        self.finish_reason.as_deref() == Some("length")
    }
}

/// One chat turn with a proper role (§20 multi-turn history).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChatTurn {
    pub role: String, // system | user | assistant | tool
    pub content: String,
    /// Stage 19: JPEG data URLs appended as image_url parts (§33).
    #[serde(default, skip_serializing)]
    pub images: Vec<String>,
}

impl ChatTurn {
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            images: vec![],
        }
    }
}

/// Rewrite a turn list into the shape the loaded model's template accepts.
///
/// Gemma 2's template raises `System role not supported` on a system turn and
/// `Conversation roles must alternate user/assistant/...` on anything else,
/// which llama-server returns as a 400 before a single token is generated: the
/// model was unusable in this app, not merely degraded. Folding is preferred
/// to dropping, so the instructions still reach a model that cannot be given
/// them in their own turn.
///
/// Permissive templates (nearly all of them) are returned untouched, so the
/// prompt prefix stays byte-identical and the KV cache still hits.
fn template_safe_turns(turns: &[ChatTurn], shape: ChatTemplateShape) -> Vec<ChatTurn> {
    if !shape.is_restricted() {
        return turns.to_vec();
    }
    let mut out: Vec<ChatTurn> = Vec::with_capacity(turns.len());
    let mut carried = String::new();
    for turn in turns {
        let mut role = turn.role.as_str();
        // A tool result is an observation for the model to read, so it belongs
        // with the user side; no restricted template defines a tool turn.
        if shape.strict_alternation && role == "tool" {
            role = "user";
        }
        if role == "system" && !shape.system_role {
            if !carried.is_empty() {
                carried.push_str("\n\n");
            }
            carried.push_str(&turn.content);
            continue;
        }
        let mut content = turn.content.clone();
        if !carried.is_empty() && role == "user" {
            content = if content.trim().is_empty() {
                std::mem::take(&mut carried)
            } else {
                format!("{}\n\n{}", std::mem::take(&mut carried), content)
            };
        }
        // Merge into the previous turn when the roles would repeat, and open
        // with a user turn: both are what strict alternation demands.
        let repeats = out.last().map(|last: &ChatTurn| last.role == role) == Some(true);
        let leads_with_assistant = out.is_empty() && role != "user";
        if shape.strict_alternation && (repeats || leads_with_assistant) {
            if let Some(last) = out.last_mut() {
                if !last.content.trim().is_empty() && !content.trim().is_empty() {
                    last.content.push_str("\n\n");
                }
                last.content.push_str(&content);
                last.images.extend(turn.images.iter().cloned());
                continue;
            }
            // An assistant turn with nothing before it becomes the user's.
            role = "user";
        }
        out.push(ChatTurn {
            role: role.to_string(),
            content,
            images: turn.images.clone(),
        });
    }
    // System text with no user turn after it still has to travel.
    if !carried.is_empty() {
        match out.iter_mut().find(|turn| turn.role == "user") {
            Some(turn) => turn.content = format!("{carried}\n\n{}", turn.content),
            None => out.insert(0, ChatTurn::text("user", carried)),
        }
    }
    out
}

/// Whether a rejected request was rejected by the chat template rather than
/// by its content. The template raises; llama-server reports the raised text.
fn is_template_refusal(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("must alternate")
        || lower.contains("role not supported")
        || lower.contains("unable to generate parser for this template")
        || (lower.contains("template") && lower.contains("jinja"))
}

/// Classify a non-success llama-server response by its HTTP status and the
/// error object it returns (`{"error": {"code", "message", "type", ...}}`).
pub(crate) fn classify_failure(status: u16, body: &str) -> SidecarFailure {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    match parsed.as_ref().and_then(|v| v.get("error")).filter(|e| e.is_object()) {
        Some(error) => classify_error_object(error, status),
        None => classify_status(status, body.chars().take(300).collect()),
    }
}

/// An error object from a response body or from a mid-stream SSE frame.
fn classify_error_object(error: &serde_json::Value, status: u16) -> SidecarFailure {
    let kind = error.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let message: String = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .chars()
        .take(300)
        .collect();
    let code = error
        .get("code")
        .and_then(|c| c.as_u64())
        .and_then(|c| u16::try_from(c).ok())
        .unwrap_or(status);
    let count = |key: &str| {
        error
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v.min(u64::from(u32::MAX)) as u32)
            .unwrap_or(0)
    };
    match kind {
        "exceed_context_size_error" => SidecarFailure::ContextExceeded {
            prompt_tokens: count("n_prompt_tokens"),
            context: count("n_ctx"),
        },
        "unavailable_error" => SidecarFailure::Unavailable(message),
        _ => classify_status(code, message),
    }
}

fn classify_status(status: u16, detail: String) -> SidecarFailure {
    match status {
        503 => SidecarFailure::Unavailable(detail),
        408 | 504 => SidecarFailure::Timeout,
        400..=499 => SidecarFailure::BadRequest(detail),
        _ => SidecarFailure::Server(detail),
    }
}

/// No response arrived: a timeout, or a connection that could not be made or
/// was reset.
fn transport_failure(error: &reqwest::Error) -> SidecarFailure {
    if error.is_timeout() {
        SidecarFailure::Timeout
    } else {
        SidecarFailure::Unavailable(error.to_string())
    }
}

/// The same message list under the strictest shape any template demands.
/// The GGUF template is read at load, but a model can ship without one, or
/// refuse in wording this host does not recognize; one retry then rescues a
/// model that would otherwise answer 400 to every request it serves.
/// None when nothing would change, or when parts (images) cannot be folded.
fn strictly_shaped_body(body: &serde_json::Value) -> Option<serde_json::Value> {
    let messages = body.get("messages")?.as_array()?;
    let mut turns = Vec::with_capacity(messages.len());
    for message in messages {
        turns.push(ChatTurn::text(
            message.get("role")?.as_str()?,
            message.get("content")?.as_str()?,
        ));
    }
    let strict = template_safe_turns(
        &turns,
        ChatTemplateShape {
            system_role: false,
            strict_alternation: true,
        },
    );
    if strict.len() == turns.len()
        && strict
            .iter()
            .zip(&turns)
            .all(|(a, b)| a.role == b.role && a.content == b.content)
    {
        return None;
    }
    let mut reshaped = body.clone();
    reshaped["messages"] = strict.iter().map(turn_json).collect::<Vec<_>>().into();
    Some(reshaped)
}

fn turn_json(t: &ChatTurn) -> serde_json::Value {
    if t.images.is_empty() {
        return serde_json::json!({"role": t.role, "content": t.content});
    }
    let mut parts = vec![serde_json::json!({"type": "text", "text": t.content})];
    for url in &t.images {
        parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": url}}));
    }
    serde_json::json!({"role": t.role, "content": parts})
}

/// Per-request knobs on top of the loaded model's sampling configuration.
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    /// `Some(false)` asks the chat template to skip native thinking (Qwen 3,
    /// Gemma 4 and similar); `None` leaves the template default untouched.
    pub thinking: Option<bool>,
    /// Runtime-enforced JSON schema (`response_format`) when set.
    pub response_format: Option<serde_json::Value>,
    /// Greedy decoding for machine-readable decisions.
    pub deterministic: bool,
}

fn sampling_body(
    turns: &[ChatTurn],
    max_tokens: u32,
    cfg: &InferenceConfig,
    stream: bool,
) -> serde_json::Value {
    let turns = template_safe_turns(turns, cfg.chat_template);
    serde_json::json!({
        "messages": turns.iter().map(turn_json).collect::<Vec<_>>(),
        "max_tokens": max_tokens,
        "temperature": cfg.temperature,
        "top_p": cfg.top_p,
        "top_k": cfg.top_k,
        "repeat_penalty": cfg.repeat_penalty,
        "stream": stream,
        // Explicit: the slot keeps the longest common prefix of the previous
        // prompt in its KV cache, so a growing transcript only prefills its tail.
        "cache_prompt": true,
    })
}

fn apply_options(body: &mut serde_json::Value, options: &RequestOptions) {
    if let Some(thinking) = options.thinking {
        body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": thinking});
    }
    if let Some(format) = &options.response_format {
        body["response_format"] = format.clone();
    }
    if options.deterministic {
        body["temperature"] = serde_json::json!(0);
    }
}

/// Observed delivery timing, not an estimate of hidden model compute. Only
/// nonempty content can open an output span; reasoning closes that span.
#[derive(Default)]
struct VisibleOutputClock {
    first_visible: Option<u64>,
    segment_start: Option<u64>,
    last_visible: Option<u64>,
    output_ms: u64,
    reasoning_start: Option<u64>,
    thinking_ms: Option<u64>,
}

impl VisibleOutputClock {
    fn close_output(&mut self) {
        if let (Some(start), Some(end)) = (self.segment_start.take(), self.last_visible) {
            self.output_ms = self.output_ms.saturating_add(end.saturating_sub(start));
        }
    }

    fn reasoning(&mut self, now: u64) {
        self.close_output();
        self.reasoning_start.get_or_insert(now);
        self.thinking_ms.get_or_insert(0);
    }

    fn visible(&mut self, now: u64) {
        if let Some(start) = self.reasoning_start.take() {
            self.thinking_ms = Some(
                self.thinking_ms
                    .unwrap_or(0)
                    .saturating_add(now.saturating_sub(start)),
            );
        }
        self.first_visible.get_or_insert(now);
        self.segment_start.get_or_insert(now);
        self.last_visible = Some(now);
    }

    fn finish(mut self, total_ms: u64) -> OutputTiming {
        self.close_output();
        if let Some(start) = self.reasoning_start.take() {
            self.thinking_ms = Some(
                self.thinking_ms
                    .unwrap_or(0)
                    .saturating_add(total_ms.saturating_sub(start)),
            );
        }
        OutputTiming {
            first_visible_ms: self.first_visible,
            output_ms: self.output_ms,
            thinking_ms: self.thinking_ms,
            total_ms,
            ..OutputTiming::default()
        }
    }
}

/// Callbacks for one streamed completion. Every handler is optional; the
/// defaults do nothing, never cancel and never stop early.
pub struct StreamHandlers {
    pub on_token: Box<dyn FnMut(&str) + Send>,
    pub on_reasoning: Box<dyn FnMut(&str) + Send>,
    pub on_phase: Box<dyn FnMut(&'static str) + Send>,
    pub is_cancelled: Box<dyn Fn() -> bool + Send + Sync>,
    /// Inspects the visible text so far; `true` stops reading. The runtime
    /// drops the slot as soon as the connection closes, so tokens after a
    /// complete action are never generated at all.
    pub should_stop: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

impl Default for StreamHandlers {
    fn default() -> Self {
        Self {
            on_token: Box::new(|_| {}),
            on_reasoning: Box::new(|_| {}),
            on_phase: Box::new(|_| {}),
            is_cancelled: Box::new(|| false),
            should_stop: Box::new(|_| false),
        }
    }
}

/// Everything one streamed completion produced, with the runtime's own
/// accounting where it reported it.
#[derive(Debug, Clone, Default)]
pub struct StreamOutcome {
    pub text: String,
    pub metrics: Metrics,
    pub finish_reason: Option<String>,
    pub reasoning_present: bool,
    pub native_tool_calls_present: bool,
    pub early_stopped: bool,
    pub cancelled: bool,
}

impl StreamOutcome {
    pub fn into_completion(self) -> AgentCompletion {
        AgentCompletion {
            text: self.text,
            finish_reason: if self.early_stopped {
                Some("stop".into())
            } else {
                self.finish_reason
            },
            reasoning_present: self.reasoning_present,
            reasoning_tokens: None,
            native_tool_calls_present: self.native_tool_calls_present,
            early_stopped: self.early_stopped,
            metrics: self.metrics,
        }
    }
}

/// Incremental SSE frame splitter: frames end at a blank line (LF or CRLF).
/// Scanning resumes where the previous pass stopped instead of rescanning the
/// whole buffer for every network chunk.
struct SseFrames {
    buf: Vec<u8>,
    scan_from: usize,
}

impl SseFrames {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            scan_from: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn next_frame(&mut self) -> Option<Vec<u8>> {
        let start = self.scan_from.saturating_sub(3);
        let mut end = None;
        let mut index = start;
        while index < self.buf.len() {
            if self.buf[index] == b'\n' {
                if self.buf.get(index + 1) == Some(&b'\n') {
                    end = Some((index, 2));
                    break;
                }
                if self.buf.get(index + 1) == Some(&b'\r') && self.buf.get(index + 2) == Some(&b'\n')
                {
                    end = Some((index, 3));
                    break;
                }
            }
            index += 1;
        }
        let (at, separator) = end?;
        let frame: Vec<u8> = self.buf.drain(..at + separator).collect();
        self.scan_from = 0;
        Some(frame)
    }

    fn mark_scanned(&mut self) {
        self.scan_from = self.buf.len();
    }
}

impl SidecarClient {
    pub fn new(base_url: String) -> Result<Self, InferenceError> {
        Ok(Self {
            base_url,
            inner: shared_client().clone(),
            recorder: None,
        })
    }

    /// The template capabilities the runtime reports, or None when it does
    /// not report them or does not answer within a few seconds.
    pub async fn template_caps(&self) -> Option<crate::inference::TemplateCaps> {
        let response = self
            .inner
            .get(format!("{}/props", self.base_url))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let props: serde_json::Value = response.json().await.ok()?;
        crate::inference::TemplateCaps::from_props(&props)
    }

    /// Every request this client sends is handed to `recorder` once it ends.
    pub fn with_recorder(mut self, recorder: RequestRecorder) -> Self {
        self.recorder = Some(recorder);
        self
    }

    fn record_request(
        &self,
        body: &serde_json::Value,
        result: Result<(&str, Option<&str>, &'static str, &Metrics), &InferenceError>,
    ) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        let request = match result {
            Ok((output, finish_reason, outcome, metrics)) => RecordedRequest {
                body: without_image_data(body),
                output: output.to_string(),
                finish_reason: finish_reason.map(str::to_string),
                outcome,
                failure: None,
                prompt_tokens: metrics.prompt_tokens,
                cached_tokens: metrics.engine.as_ref().map(|engine| engine.cached_tokens).unwrap_or(0),
                generated_tokens: metrics.generated_tokens,
            },
            Err(error) => RecordedRequest {
                body: without_image_data(body),
                output: String::new(),
                finish_reason: None,
                outcome: "failed",
                failure: Some(error.to_string()),
                prompt_tokens: 0,
                cached_tokens: 0,
                generated_tokens: 0,
            },
        };
        recorder(request);
    }

    /// Non-streaming chat completion. Returns text + usage-derived metrics.
    pub async fn chat(
        &self,
        prompt: &str,
        max_tokens: u32,
        cfg: &InferenceConfig,
    ) -> Result<(String, Metrics), InferenceError> {
        self.chat_turns(&[ChatTurn::text("user", prompt)], max_tokens, cfg)
            .await
    }

    pub async fn chat_turns(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
    ) -> Result<(String, Metrics), InferenceError> {
        let body = sampling_body(turns, max_tokens, cfg, false);
        self.complete(body, cfg).await
    }

    /// The enforced action envelope for models whose template has no tool
    /// support. Each tool is its own alternative, with its name fixed and the
    /// arguments it cannot run without required (`tools::required_args`), so
    /// the runtime's grammar cannot produce a blank name or a call its tool
    /// will refuse. Measured live under the looser envelope: a 4B model sent a
    /// tool call with an empty name, a search without a query and a file write
    /// without its text.
    pub(crate) fn structured_action_schema() -> serde_json::Value {
        // Field order is generation order (serde_json keeps insertion order):
        // kind and name first, so the model has chosen the tool before it writes
        // arguments. Serialized alphabetically, answer and args came first and
        // name last; measured live, a 4B model wrote a whole page as `content`
        // and then named the tool list_directory, which the grammar allowed.
        let mut alternatives = vec![serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {"const": "final"},
                "name": {"const": ""},
                "args": {"type": "object", "properties": {}, "additionalProperties": false},
                "answer": {"type": "string", "minLength": 1}
            },
            "required": ["kind", "name", "args", "answer"],
            "additionalProperties": false
        })];
        for tool in crate::tools::registry() {
            let required = crate::tools::required_args(tool.name);
            let properties: serde_json::Map<String, serde_json::Value> = required
                .iter()
                .map(|(name, non_empty)| {
                    let schema = if *non_empty {
                        serde_json::json!({"type": "string", "minLength": 1})
                    } else {
                        serde_json::json!({"type": "string"})
                    };
                    (name.to_string(), schema)
                })
                .collect();
            let names: Vec<&str> = required.iter().map(|(name, _)| *name).collect();
            alternatives.push(serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "tool"},
                    "name": {"const": tool.name},
                    "args": {"type": "object", "properties": properties, "required": names, "additionalProperties": true},
                    "answer": {"const": ""}
                },
                "required": ["kind", "name", "args", "answer"],
                "additionalProperties": false
            }));
        }
        serde_json::json!({"type": "json_object", "schema": {"anyOf": alternatives}})
    }

    /// Only agent execution opts into this override. A caller may make one
    /// bounded recovery attempt after budget exhaustion, then retain the flag
    /// for that run. This method never retries invisibly or changes Chat policy.
    pub async fn agent_chat_turns(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
        disable_native_thinking: bool,
        structured: bool,
    ) -> Result<AgentCompletion, InferenceError> {
        let mut body = sampling_body(turns, max_tokens, cfg, false);
        apply_options(
            &mut body,
            &RequestOptions {
                thinking: disable_native_thinking.then_some(false),
                // The caller adds the format instruction before estimating its
                // request context. Never append invisible/unaccounted turns here.
                response_format: structured.then(Self::structured_action_schema),
                deterministic: false,
            },
        );
        self.complete_detailed(body, cfg).await
    }

    /// Streaming variant of `agent_chat_turns`: visible deltas reach the
    /// handlers as they arrive, and `should_stop` can end the request the
    /// moment a complete action has been received.
    pub async fn agent_chat_turns_stream(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
        disable_native_thinking: bool,
        structured: bool,
        handlers: StreamHandlers,
    ) -> Result<AgentCompletion, InferenceError> {
        let options = RequestOptions {
            thinking: disable_native_thinking.then_some(false),
            response_format: structured.then(Self::structured_action_schema),
            deterministic: false,
        };
        let outcome = self
            .stream(turns, max_tokens, cfg, &options, handlers)
            .await?;
        Ok(outcome.into_completion())
    }

    /// Host-side completion checks need a short machine-readable decision, not
    /// the model's native reasoning channel consuming the entire answer budget.
    /// This override is deliberately separate from user chat/reasoning requests.
    pub async fn chat_turns_without_reasoning(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
    ) -> Result<(String, Metrics), InferenceError> {
        let mut body = sampling_body(turns, max_tokens, cfg, false);
        apply_options(
            &mut body,
            &RequestOptions {
                thinking: Some(false),
                ..RequestOptions::default()
            },
        );
        self.complete(body, cfg).await
    }

    async fn complete(
        &self,
        body: serde_json::Value,
        cfg: &InferenceConfig,
    ) -> Result<(String, Metrics), InferenceError> {
        let completion = self.complete_detailed(body, cfg).await?;
        Ok((completion.text, completion.metrics))
    }

    /// One immediate retry when the request could not even be sent. A
    /// keep-alive connection the server closed after an early-stopped stream
    /// fails on its next use before any bytes are exchanged; nothing has been
    /// generated yet, so repeating the request is free and invisible.
    async fn send_with_retry(
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, InferenceError> {
        let retry = builder.try_clone();
        match builder.send().await {
            Ok(response) => Ok(response),
            Err(error) if !error.is_timeout() && (error.is_connect() || error.is_request()) => {
                let Some(retry) = retry else {
                    return Err(InferenceError::Sidecar(transport_failure(&error)));
                };
                tracing::warn!("sidecar request failed before any response ({error}); retrying once");
                retry
                    .send()
                    .await
                    .map_err(|e| InferenceError::Sidecar(transport_failure(&e)))
            }
            Err(error) => Err(InferenceError::Sidecar(transport_failure(&error))),
        }
    }

    /// Send one chat request, retrying once under the strictest message shape
    /// if the template refused the list. Nothing has been generated when a
    /// template raises, so the retry repeats no visible work.
    async fn post_chat(
        &self,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, InferenceError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let r = Self::send_with_retry(self.inner.post(&url).json(body)).await?;
        if r.status().is_success() {
            return Ok(r);
        }
        let code = r.status();
        let text = r.text().await.unwrap_or_default();
        if code == reqwest::StatusCode::BAD_REQUEST && is_template_refusal(&text) {
            if let Some(reshaped) = strictly_shaped_body(body) {
                tracing::warn!(
                    "chat template refused the message list ({}); retrying with system text folded into the first user turn and roles alternating",
                    text.chars().take(160).collect::<String>()
                );
                let retry = Self::send_with_retry(self.inner.post(&url).json(&reshaped)).await?;
                if retry.status().is_success() {
                    return Ok(retry);
                }
                let code = retry.status();
                let text = retry.text().await.unwrap_or_default();
                tracing::warn!(
                    "llama-server returned {code} for both the original and the template-safe message list: {}",
                    text.chars().take(300).collect::<String>()
                );
                return Err(InferenceError::Sidecar(classify_failure(code.as_u16(), &text)));
            }
        }
        Err(InferenceError::Sidecar(classify_failure(code.as_u16(), &text)))
    }

    async fn complete_detailed(
        &self,
        body: serde_json::Value,
        cfg: &InferenceConfig,
    ) -> Result<AgentCompletion, InferenceError> {
        let result = self.complete_detailed_body(&body, cfg).await;
        match &result {
            Ok(completion) => {
                observe_prompt_size(&body, completion.metrics.prompt_tokens);
                self.record_request(
                    &body,
                    Ok((&completion.text, completion.finish_reason.as_deref(), "completed", &completion.metrics)),
                )
            }
            Err(error) => self.record_request(&body, Err(error)),
        }
        result
    }

    async fn complete_detailed_body(
        &self,
        body: &serde_json::Value,
        cfg: &InferenceConfig,
    ) -> Result<AgentCompletion, InferenceError> {
        let r = self.post_chat(body).await?;
        let v: serde_json::Value = r
            .json()
            .await
            .map_err(|e| InferenceError::Sidecar(SidecarFailure::Server(format!("unreadable response: {e}"))))?;
        let text = v
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let usage = &v["usage"];
        let reasoning_tokens = usage
            .pointer("/completion_tokens_details/reasoning_tokens")
            .and_then(|tokens| tokens.as_u64())
            .map(|tokens| tokens.min(u32::MAX as u64) as u32);
        let message = &v["choices"][0]["message"];
        let reasoning_present = reasoning_tokens.is_some_and(|tokens| tokens > 0)
            || message
                .get("reasoning_content")
                .and_then(|text| text.as_str())
                .is_some_and(|text| !text.trim().is_empty());
        let finish_reason = v
            .pointer("/choices/0/finish_reason")
            .and_then(|reason| reason.as_str())
            .map(str::to_owned);
        let native_tool_calls_present = message
            .get("tool_calls")
            .and_then(|calls| calls.as_array())
            .is_some_and(|calls| !calls.is_empty())
            || message
                .get("function_call")
                .is_some_and(|call| call.is_object())
            || matches!(
                finish_reason.as_deref(),
                Some("tool_calls" | "function_call")
            );
        let engine = crate::inference::EngineTimings::from_json(&v["timings"]);
        Ok(AgentCompletion {
            text,
            metrics: Metrics {
                tokens_per_sec: engine
                    .as_ref()
                    .and_then(|e| e.predicted_tps)
                    .unwrap_or(0.0) as f32,
                prompt_speed_tps: engine
                    .as_ref()
                    .and_then(|e| e.prompt_tps)
                    .unwrap_or(0.0) as f32,
                time_to_first_token_ms: engine
                    .as_ref()
                    .map(|e| e.prompt_ms.max(0.0) as u64)
                    .unwrap_or(0),
                prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                generated_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
                kv_cache_used: 0,
                kv_cache_limit: cfg.n_ctx,
                timing: None,
                engine,
                finish_reason: finish_reason.clone(),
            },
            finish_reason,
            reasoning_present,
            reasoning_tokens,
            native_tool_calls_present,
            early_stopped: false,
        })
    }

    /// Streaming chat: forwards `content` deltas as SSE `data:` chunks arrive.
    /// Calls `on_token` per delta; honors `is_cancelled` between chunks.
    /// Callbacks are owned (`'static`) so callers can `tokio::spawn` the stream.
    pub async fn chat_stream(
        &self,
        prompt: &str,
        max_tokens: u32,
        cfg: &InferenceConfig,
        on_token: impl FnMut(String) + Send + 'static,
        is_cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Metrics, InferenceError> {
        self.chat_turns_stream(
            &[ChatTurn::text("user", prompt)],
            max_tokens,
            cfg,
            on_token,
            is_cancelled,
        )
        .await
    }

    pub async fn chat_turns_stream(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
        on_token: impl FnMut(String) + Send + 'static,
        is_cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Metrics, InferenceError> {
        self.chat_turns_stream_observed(turns, max_tokens, cfg, on_token, |_| {}, is_cancelled)
            .await
    }

    pub async fn chat_turns_stream_observed(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
        mut on_token: impl FnMut(String) + Send + 'static,
        on_phase: impl FnMut(&'static str) + Send + 'static,
        is_cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Metrics, InferenceError> {
        let handlers = StreamHandlers {
            on_token: Box::new(move |delta| on_token(delta.to_string())),
            on_phase: Box::new(on_phase),
            is_cancelled: Box::new(is_cancelled),
            ..StreamHandlers::default()
        };
        self.stream(turns, max_tokens, cfg, &RequestOptions::default(), handlers)
            .await
            .map(|outcome| outcome.metrics)
    }

    /// The streaming core. Visible content and native reasoning are routed to
    /// separate handlers; the runtime's final `timings`/`usage` frame supplies
    /// engine-measured numbers; `should_stop` can release the slot early.
    pub async fn stream(
        &self,
        turns: &[ChatTurn],
        max_tokens: u32,
        cfg: &InferenceConfig,
        options: &RequestOptions,
        handlers: StreamHandlers,
    ) -> Result<StreamOutcome, InferenceError> {
        let request_started = std::time::Instant::now();
        let mut body = sampling_body(turns, max_tokens, cfg, true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
        // Every chunk carries the engine's timings, so a stream that is closed
        // early (a complete action arrived) still reports prompt processing,
        // cache hits and the tokens decoded so far.
        body["timings_per_token"] = serde_json::json!(true);
        apply_options(&mut body, options);
        let result = self.stream_body(&body, cfg, handlers, request_started).await;
        match &result {
            Ok(outcome) => {
                observe_prompt_size(&body, outcome.metrics.prompt_tokens);
                let kind = if outcome.cancelled {
                    "cancelled"
                } else if outcome.early_stopped {
                    "early_stopped"
                } else {
                    "completed"
                };
                self.record_request(
                    &body,
                    Ok((&outcome.text, outcome.finish_reason.as_deref(), kind, &outcome.metrics)),
                );
            }
            Err(error) => self.record_request(&body, Err(error)),
        }
        result
    }

    async fn stream_body(
        &self,
        body: &serde_json::Value,
        cfg: &InferenceConfig,
        mut handlers: StreamHandlers,
        request_started: std::time::Instant,
    ) -> Result<StreamOutcome, InferenceError> {
        use futures::StreamExt;
        (handlers.on_phase)("processing");
        let mut phase = "processing";
        let r = self.post_chat(body).await?;
        let mut outcome = StreamOutcome {
            metrics: Metrics {
                kv_cache_limit: cfg.n_ctx,
                ..Metrics::default()
            },
            ..StreamOutcome::default()
        };
        let mut frames = SseFrames::new();
        let mut clock = VisibleOutputClock::default();
        let mut usage_seen = false;
        // A stream the server finished ends with a finish_reason and [DONE];
        // one that just stops (a crashed or restarted server) is truncated.
        let mut done_seen = false;
        let mut byte_stream = r.bytes_stream();
        'read: while let Some(chunk) = byte_stream.next().await {
            if (handlers.is_cancelled)() {
                outcome.cancelled = true;
                break;
            }
            let bytes = chunk.map_err(|e| InferenceError::Sidecar(transport_failure(&e)))?;
            // Decode only complete SSE frames, preserving UTF-8 characters
            // when the transport splits a code point between network chunks.
            frames.push(&bytes);
            while let Some(frame_bytes) = frames.next_frame() {
                let frame = String::from_utf8_lossy(&frame_bytes);
                for line in frame.lines() {
                    let line = line.strip_prefix("data:").map(str::trim).unwrap_or("");
                    if line == "[DONE]" {
                        done_seen = true;
                        continue;
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                        continue;
                    };
                    // An error raised after streaming began arrives as its own
                    // frame; skipping it made a failed reply look finished.
                    if let Some(error) = v.get("error").filter(|error| error.is_object()) {
                        return Err(InferenceError::Sidecar(classify_error_object(error, 500)));
                    }
                    let now = request_started.elapsed().as_millis() as u64;
                    let choice = &v["choices"][0];
                    if let Some(reasoning) = choice
                        .pointer("/delta/reasoning_content")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                    {
                        outcome.reasoning_present = true;
                        clock.reasoning(now);
                        if phase != "thinking" {
                            phase = "thinking";
                            (handlers.on_phase)(phase);
                        }
                        (handlers.on_reasoning)(reasoning);
                    }
                    if choice
                        .pointer("/delta/tool_calls")
                        .and_then(|calls| calls.as_array())
                        .is_some_and(|calls| !calls.is_empty())
                    {
                        outcome.native_tool_calls_present = true;
                    }
                    if let Some(delta) = choice
                        .pointer("/delta/content")
                        .and_then(|c| c.as_str())
                        .filter(|delta| !delta.is_empty())
                    {
                        clock.visible(now);
                        if phase != "responding" {
                            phase = "responding";
                            (handlers.on_phase)(phase);
                        }
                        outcome.text.push_str(delta);
                        (handlers.on_token)(delta);
                        if (handlers.should_stop)(&outcome.text) {
                            outcome.early_stopped = true;
                            break 'read;
                        }
                    }
                    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                        outcome.finish_reason = Some(reason.to_owned());
                        if matches!(reason, "tool_calls" | "function_call") {
                            outcome.native_tool_calls_present = true;
                        }
                    }
                    if let Some(u) = v.get("usage").filter(|usage| usage.is_object()) {
                        outcome.metrics.prompt_tokens =
                            u["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                        if let Some(tokens) = u["completion_tokens"].as_u64() {
                            outcome.metrics.generated_tokens = tokens.min(u32::MAX as u64) as u32;
                            usage_seen = true;
                        }
                    }
                    if let Some(engine) = crate::inference::EngineTimings::from_json(&v["timings"])
                    {
                        outcome.metrics.engine = Some(engine);
                    }
                }
            }
            frames.mark_scanned();
        }
        // Dropping the response stream closes the connection: llama-server
        // aborts the slot instead of finishing tokens nobody will read.
        drop(byte_stream);
        if !outcome.cancelled && !outcome.early_stopped && !done_seen && outcome.finish_reason.is_none() {
            return Err(InferenceError::Sidecar(SidecarFailure::Truncated));
        }
        let mut timing = clock.finish(request_started.elapsed().as_millis() as u64);
        if let Some(engine) = &outcome.metrics.engine {
            timing.add_engine(engine);
        }
        if !outcome.text.is_empty() {
            // The engine's own count is exact for the visible text when no
            // hidden reasoning shared the budget; otherwise count only the
            // delivered text, and never let hidden tokens inflate the rate.
            let engine_count = outcome
                .metrics
                .engine
                .as_ref()
                .filter(|_| !outcome.reasoning_present && !outcome.early_stopped)
                .map(|e| e.predicted_tokens)
                .filter(|count| *count > 0);
            let exact = match engine_count {
                Some(count) => Some((count, "engine")),
                None if outcome.text.len() <= 4_000_000 => self
                    .visible_token_count(&outcome.text)
                    .await
                    .map(|count| (count, "tokenizer")),
                None => None,
            };
            timing.output_tokens = exact.map(|(count, _)| count).unwrap_or_else(|| {
                outcome
                    .text
                    .chars()
                    .count()
                    .div_ceil(4)
                    .min(u32::MAX as usize) as u32
            });
            timing.estimated = exact.is_none();
            timing.token_basis = exact
                .map(|(_, basis)| basis)
                .unwrap_or("character_estimate")
                .into();
        }
        timing.update_rate();
        if !usage_seen {
            // The final usage frame never arrived (early stop or cancel): the
            // engine's own counters are exact for what was processed.
            outcome.metrics.generated_tokens = outcome
                .metrics
                .engine
                .as_ref()
                .map(|e| e.predicted_tokens)
                .unwrap_or(timing.output_tokens);
            if let Some(engine) = &outcome.metrics.engine {
                outcome.metrics.prompt_tokens = engine.prompt_tokens.saturating_add(engine.cached_tokens);
            }
        }
        outcome.metrics.time_to_first_token_ms = timing.first_visible_ms.unwrap_or(0);
        outcome.metrics.tokens_per_sec = timing
            .engine_output_tps
            .or(timing.output_tps)
            .unwrap_or(0.0) as f32;
        outcome.metrics.prompt_speed_tps = timing.engine_prompt_tps.unwrap_or(0.0) as f32;
        outcome.metrics.finish_reason = outcome.finish_reason.clone();
        outcome.metrics.timing = Some(timing);
        Ok(outcome)
    }

    /// A bounded local tokenizer request counts only content delivered to the
    /// user, never usage.completion_tokens (which can include native reasoning).
    async fn visible_token_count(&self, text: &str) -> Option<u32> {
        let response = self
            .inner
            .post(format!("{}/tokenize", self.base_url))
            .timeout(Duration::from_millis(750))
            .json(&serde_json::json!({"content": text, "add_special": false}))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: serde_json::Value = response.json().await.ok()?;
        let tokens = value.get("tokens")?.as_array()?;
        if tokens.is_empty() || !tokens.iter().all(|token| token.as_u64().is_some()) {
            return None;
        }
        u32::try_from(tokens.len()).ok()
    }

    pub async fn tokenize(&self, text: &str) -> Result<Vec<u32>, InferenceError> {
        let r = self
            .inner
            .post(format!("{}/tokenize", self.base_url))
            .json(&serde_json::json!({"content": text}))
            .send()
            .await
            .map_err(|e| InferenceError::Generation(format!("tokenize failed: {e}")))?;
        let v: serde_json::Value = r
            .json()
            .await
            .map_err(|e| InferenceError::Generation(format!("bad tokenize JSON: {e}")))?;
        Ok(v["tokens"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_u64().map(|n| n as u32))
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Manager held in AppState: at most one running sidecar (§15 single model).
pub struct LlamaServerManager {
    pub running: Option<RunningSidecar>,
    pub last_error: Option<String>,
}

impl LlamaServerManager {
    pub fn new() -> Self {
        Self {
            running: None,
            last_error: None,
        }
    }

    pub fn is_running(&mut self) -> bool {
        match &mut self.running {
            Some(r) => r.is_alive(),
            None => false,
        }
    }

    pub fn base_url(&self) -> Option<String> {
        self.running.as_ref().map(|r| r.base_url.clone())
    }

    pub async fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.stop().await;
        }
    }
}

impl Default for LlamaServerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceStatus {
    pub runtime_notice: Option<String>,
    pub engine: String, // "stub" | "llama-server"
    pub running: bool,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub context_size: u32,
    pub binary_found: bool,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod replay_fixture_tests {
    use super::*;

    fn failure_class(failure: &SidecarFailure) -> &'static str {
        match failure {
            SidecarFailure::ContextExceeded { .. } => "context_exceeded",
            SidecarFailure::Unavailable(_) => "unavailable",
            SidecarFailure::Timeout => "timeout",
            SidecarFailure::Truncated => "truncated",
            SidecarFailure::BadRequest(_) => "bad_request",
            SidecarFailure::Server(_) => "server",
        }
    }

    /// Serve `body` in `chunk`-byte pieces with the given status.
    async fn serve(body: Vec<u8>, status: u16, content_type: String, chunk: usize) -> (String, tokio::task::JoinHandle<()>) {
        use futures::StreamExt;
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let body = body.clone();
                let content_type = content_type.clone();
                async move {
                    let pieces: Vec<Vec<u8>> = body.chunks(chunk.max(1)).map(|piece| piece.to_vec()).collect();
                    let stream = futures::stream::iter(pieces).map(Ok::<_, std::convert::Infallible>);
                    axum::http::Response::builder()
                        .status(status)
                        .header("content-type", content_type)
                        .body(axum::body::Body::from_stream(stream))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{port}"), server)
    }

    #[tokio::test]
    async fn every_recorded_stream_replays_to_its_expected_outcome() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/streams");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("fixture folder")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.strip_suffix(".sse")).map(str::to_string))
            .collect();
        names.sort();
        assert!(names.len() >= 9, "fixtures missing: {names:?}");
        for name in &names {
            let body = std::fs::read(dir.join(format!("{name}.sse"))).unwrap();
            let expect: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.expect.json"))).unwrap()).unwrap();
            let status = expect["status"].as_u64().unwrap_or(200) as u16;
            let content_type = expect["content_type"].as_str().unwrap_or("text/event-stream").to_string();
            for chunk in [1usize, 7, body.len()] {
                let (url, server) = serve(body.clone(), status, content_type.clone(), chunk).await;
                // No /tokenize here: token counts fall back to the engine's.
                let result = SidecarClient::new(url)
                    .unwrap()
                    .stream(
                        &[ChatTurn::text("user", "fixture")],
                        256,
                        &InferenceConfig::default(),
                        &RequestOptions::default(),
                        StreamHandlers::default(),
                    )
                    .await;
                server.abort();
                let context = format!("{name} in {chunk}-byte chunks");
                match (expect.get("error").and_then(|e| e.as_str()), result) {
                    (Some(class), Err(error)) => {
                        let failure = error.sidecar().unwrap_or_else(|| panic!("{context}: untyped error {error}"));
                        assert_eq!(failure_class(failure), class, "{context}");
                        if let SidecarFailure::ContextExceeded { prompt_tokens, context: window } = failure {
                            assert_eq!(u64::from(*prompt_tokens), expect["prompt_tokens"].as_u64().unwrap(), "{context}");
                            assert_eq!(u64::from(*window), expect["context"].as_u64().unwrap(), "{context}");
                        }
                    }
                    (Some(class), Ok(outcome)) => panic!("{context}: expected {class}, got text {:?}", outcome.text),
                    (None, Err(error)) => panic!("{context}: unexpected error {error}"),
                    (None, Ok(outcome)) => {
                        assert_eq!(outcome.text, expect["text"].as_str().unwrap(), "{context}");
                        assert_eq!(outcome.finish_reason.as_deref(), expect["finish_reason"].as_str(), "{context}");
                        assert_eq!(outcome.reasoning_present, expect["reasoning_present"].as_bool().unwrap(), "{context}");
                        assert_eq!(u64::from(outcome.metrics.prompt_tokens), expect["prompt_tokens"].as_u64().unwrap(), "{context}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod failure_class_tests {
    use super::*;

    #[test]
    fn a_recorded_body_keeps_the_text_and_drops_image_data() {
        let body = serde_json::json!({"messages": [{"role": "user", "content": [
            {"type": "text", "text": "what is this"},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", "A".repeat(5000))}}
        ]}]});
        let kept = without_image_data(&body);
        assert_eq!(kept["messages"][0]["content"][0]["text"], "what is this");
        let url = kept["messages"][0]["content"][1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("[image data omitted"), "{url}");
    }

    #[test]
    fn a_context_overflow_is_read_from_the_servers_error_object() {
        let body = r#"{"error":{"code":400,"message":"request (9000 tokens) exceeds the available context size (8192 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":9000,"n_ctx":8192}}"#;
        assert_eq!(
            classify_failure(400, body),
            SidecarFailure::ContextExceeded { prompt_tokens: 9000, context: 8192 }
        );
        let without_counts = r#"{"error":{"code":400,"message":"too long","type":"exceed_context_size_error"}}"#;
        assert!(matches!(classify_failure(400, without_counts), SidecarFailure::ContextExceeded { prompt_tokens: 0, context: 0 }));
    }

    #[test]
    fn unavailable_and_server_errors_are_told_apart_from_bad_requests() {
        let loading = r#"{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}"#;
        assert!(matches!(classify_failure(503, loading), SidecarFailure::Unavailable(_)));
        assert!(classify_failure(503, loading).is_transient());
        assert!(matches!(classify_failure(503, "not json"), SidecarFailure::Unavailable(_)));
        let invalid = r#"{"error":{"code":400,"message":"bad field","type":"invalid_request_error"}}"#;
        assert!(matches!(classify_failure(400, invalid), SidecarFailure::BadRequest(_)));
        assert!(!classify_failure(400, invalid).is_transient());
        let crashed = r#"{"error":{"code":500,"message":"boom","type":"server_error"}}"#;
        assert!(matches!(classify_failure(500, crashed), SidecarFailure::Server(_)));
        assert!(!SidecarFailure::ContextExceeded { prompt_tokens: 1, context: 1 }.is_transient());
    }

    #[test]
    fn failures_read_as_plain_sentences() {
        let error = InferenceError::Sidecar(SidecarFailure::ContextExceeded { prompt_tokens: 9000, context: 8192 });
        assert_eq!(error.to_string(), "The request (9000 tokens) does not fit the model's 8192-token context");
        assert_eq!(InferenceError::Sidecar(SidecarFailure::Truncated).to_string(), "The model server stopped the reply before finishing it");
    }
}

#[cfg(test)]
mod load_failure_tests {
    use super::explain_load_failure;

    #[test]
    fn loader_messages_become_causes() {
        // Verbatim from llama.cpp b10809 loading registry files.
        let packed = "E llama_model_load: error loading model: done_getting_tensors: wrong number of tensors; expected 2012, got 601";
        assert!(explain_load_failure(packed).unwrap().contains("vision or audio tower"));
        let missing = "E llama_model_load: error loading model: error loading model hyperparameters: key not found in model: gemma3.attention.layer_norm_rms_epsilon";
        assert!(explain_load_failure(missing).unwrap().contains("missing metadata"));
        let shape = "check_tensor_dims: tensor 'token_embd.weight' has wrong shape; expected   2560, 262145, got   2560, 262144";
        assert!(explain_load_failure(shape).unwrap().contains("shape"));
        assert!(explain_load_failure("CUDA error: out of memory").is_none());
        let log = "0.00.266 I srv    load_model: loading model 'm.gguf'\n0.00.684 E llama_model_load: error loading model: done_getting_tensors: wrong number of tensors\n0.01.039 E srv  llama_server: exiting due to model loading error";
        assert_eq!(super::last_runtime_error(log).as_deref(), Some("srv  llama_server: exiting due to model loading error"));
        assert_eq!(super::last_runtime_error("0.1 I all fine"), None);
        let message = format!("The model file is packaged wrongly.{}{log}", super::RUNTIME_DIAGNOSTIC_MARKER);
        assert_eq!(super::without_runtime_diagnostic(&message), "The model file is packaged wrongly.");
        assert_eq!(super::without_runtime_diagnostic("no log"), "no log");
    }
}

#[cfg(test)]
mod template_shape_tests {
    use super::{template_safe_turns, ChatTurn};
    use crate::inference::ChatTemplateShape;

    const STRICT: ChatTemplateShape = ChatTemplateShape {
        system_role: false,
        strict_alternation: true,
    };

    fn shape(turns: &[ChatTurn]) -> Vec<(String, String)> {
        turns
            .iter()
            .map(|turn| (turn.role.clone(), turn.content.clone()))
            .collect()
    }

    #[test]
    fn a_permissive_template_receives_the_turns_untouched() {
        let turns = vec![
            ChatTurn::text("system", "instructions"),
            ChatTurn::text("user", "first"),
            ChatTurn::text("user", "second"),
        ];
        assert_eq!(
            shape(&template_safe_turns(&turns, ChatTemplateShape::default())),
            shape(&turns)
        );
    }

    #[test]
    fn a_strict_template_gets_folded_system_text_and_alternating_roles() {
        let turns = vec![
            ChatTurn::text("system", "instructions"),
            ChatTurn::text("user", "write a linked list"),
            ChatTurn::text("assistant", "here it is"),
            ChatTurn::text("tool", "tool output"),
            ChatTurn::text("user", "now explain it"),
        ];
        let safe = template_safe_turns(&turns, STRICT);
        assert_eq!(
            shape(&safe),
            vec![
                ("user".into(), "instructions\n\nwrite a linked list".into()),
                ("assistant".into(), "here it is".into()),
                ("user".into(), "tool output\n\nnow explain it".into()),
            ]
        );
        // What the template actually checks: user turns on even indices.
        for (index, turn) in safe.iter().enumerate() {
            assert_eq!(turn.role == "user", index % 2 == 0, "turn {index}");
        }
    }

    #[test]
    fn system_text_still_travels_when_no_user_turn_follows_it() {
        let turns = vec![ChatTurn::text("system", "instructions")];
        assert_eq!(
            shape(&template_safe_turns(&turns, STRICT)),
            vec![("user".into(), "instructions".into())]
        );
        // An assistant preface with nothing before it cannot lead either.
        let prefaced = vec![
            ChatTurn::text("assistant", "thinking"),
            ChatTurn::text("user", "go"),
        ];
        assert_eq!(
            shape(&template_safe_turns(&prefaced, STRICT)),
            vec![("user".into(), "thinking\n\ngo".into())]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_contain_model_ctx_threads_gpu_layers() {
        let cfg = InferenceConfig {
            model_path: PathBuf::from("models/example/model.gguf"),
            n_ctx: 32768,
            n_batch: 512,
            n_threads: 16,
            n_gpu_layers: 45,
            ..InferenceConfig::default()
        };
        let a = server_args(&cfg, 3888);
        let has = |k: &str, v: &str| a.windows(2).any(|w| w[0] == k && w[1] == v);
        assert!(has("-m", "models/example/model.gguf"));
        assert!(has("--ctx-size", "32768"));
        assert!(has("--threads", "16"));
        assert!(has("--n-gpu-layers", "45"));
        assert!(has("--port", "3888"));
        assert!(a.contains(&"127.0.0.1".to_string()));
    }

    #[test]
    fn cpu_fallback_disables_all_offload_including_vision() {
        let mut cfg = InferenceConfig::default();
        cfg.projector_path = Some(PathBuf::from("vision.gguf"));
        let cpu = crate::runtime_selection::cpu_configuration(cfg, "no usable GPU");
        let args = server_args(&cpu, 3888);
        for (key, value) in [
            ("--device", "none"),
            ("--n-gpu-layers", "0"),
            ("--flash-attn", "off"),
        ] {
            assert!(args
                .windows(2)
                .any(|pair| pair[0] == key && pair[1] == value));
        }
        for flag in ["--no-op-offload", "--no-kv-offload", "--no-mmproj-offload"] {
            assert!(args.contains(&flag.to_string()));
        }
    }

    #[tokio::test]
    #[ignore = "requires idle hardware, COMPANION_CPU_PROBE_BIN and COMPANION_CPU_PROBE_MODEL"]
    async fn live_cpu_fallback_loads_and_generates_with_same_runtime() {
        let binary = PathBuf::from(std::env::var("COMPANION_CPU_PROBE_BIN").unwrap());
        let mut cfg = InferenceConfig {
            model_path: PathBuf::from(std::env::var("COMPANION_CPU_PROBE_MODEL").unwrap()),
            n_ctx: 1024,
            ..Default::default()
        };
        crate::inference::resolve_runtime_policy(None, &cfg, true).apply_to(&mut cfg);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        // Simulate no GPU discovery on the test host, then run genuine CPU
        // inference with the SAME installed binary (not a separate CPU build).
        let (sidecar, notice) = crate::runtime_selection::load_with_fallback(
            cfg,
            true,
            crate::runtime_selection::Devices::None,
            |attempt| RunningSidecar::spawn(&binary, attempt, port),
        )
        .await
        .unwrap();
        assert_eq!(sidecar.cfg.n_gpu_layers, 0);
        assert!(notice.unwrap().contains("CPU"));
        let client = SidecarClient::new(sidecar.base_url.clone()).unwrap();
        let result = client
            .chat_turns_without_reasoning(
                &[ChatTurn::text("user", "Reply with the word hello.")],
                16,
                &sidecar.cfg,
            )
            .await;
        sidecar.stop().await;
        let (answer, _) = result.unwrap();
        assert!(!answer.trim().is_empty());
        println!("Same-runtime CPU inference succeeded: {answer}");
    }

    #[test]
    fn args_include_mmproj_for_vision() {
        let cfg = InferenceConfig {
            projector_path: Some(PathBuf::from("models/v/mmproj.gguf")),
            ..InferenceConfig::default()
        };
        let a = server_args(&cfg, 3888);
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--mmproj" && w[1].ends_with("mmproj.gguf")));
    }

    #[test]
    fn args_apply_real_cache_and_auto_runtime_flags_without_matching_weights() {
        let mut cfg = InferenceConfig::default();
        let automatic = crate::inference::resolve_runtime_policy(None, &cfg, true);
        automatic.apply_to(&mut cfg);
        let args = server_args(&cfg, 3888);
        let has = |key: &str, value: &str| {
            args.windows(2)
                .any(|pair| pair[0] == key && pair[1] == value)
        };
        assert!(has("--cache-type-k", "f16"));
        assert!(has("--cache-type-v", "f16"));
        assert!(has("--flash-attn", "auto"));
        assert!(has("--n-gpu-layers", "auto"));
        assert!(has("--parallel", "1"));
        assert!(has("--cache-reuse", "256"));
        assert!(has("--spec-type", "ngram-simple"));
        assert!(
            !args.contains(&"--threads".into()),
            "automatic threads use the native default, not a literal zero"
        );
        assert!(
            !args.contains(&"--batch-size".into()),
            "automatic mode keeps the runtime's default batch sizes"
        );
        assert!(!args
            .iter()
            .any(|arg| arg.contains("slot-save") || arg.contains("chat-template")));
        cfg.flash_attn_auto = false;
        cfg.flash_attn = false;
        cfg.kv_cache_gpu = false;
        cfg.n_batch = 1024;
        cfg.speculative = "none".into();
        cfg.cache_reuse = 0;
        let manual = server_args(&cfg, 3888);
        assert!(manual
            .windows(2)
            .any(|pair| pair[0] == "--flash-attn" && pair[1] == "off"));
        assert!(manual.contains(&"--no-kv-offload".into()));
        assert!(manual
            .windows(2)
            .any(|pair| pair[0] == "--batch-size" && pair[1] == "1024"));
        assert!(!manual.contains(&"--spec-type".into()));
        assert!(!manual.contains(&"--cache-reuse".into()));
    }

    #[test]
    fn the_structured_envelope_fixes_each_tools_name_and_required_arguments() {
        let schema = SidecarClient::structured_action_schema();
        let alternatives = schema["schema"]["anyOf"].as_array().unwrap();
        assert_eq!(alternatives.len(), crate::tools::registry().len() + 1, "one per tool plus the final answer");
        assert_eq!(alternatives[0]["properties"]["kind"]["const"], "final");
        let write = alternatives.iter().find(|alternative| alternative["properties"]["name"]["const"] == "write_file").unwrap();
        assert_eq!(write["properties"]["args"]["required"], serde_json::json!(["path", "content"]));
        assert_eq!(write["properties"]["answer"]["const"], "");
        assert!(alternatives.iter().all(|alternative| alternative["properties"]["name"]["const"].is_string()));
        // Generation follows the serialized property order: the tool is chosen before its arguments.
        for alternative in alternatives {
            let order: Vec<&str> = alternative["properties"].as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(order, ["kind", "name", "args", "answer"]);
        }
        let text = schema.to_string();
        assert!(text.find("\"kind\"").unwrap() < text.find("\"answer\"").unwrap());
    }

    #[test]
    fn a_file_without_a_template_is_launched_with_its_built_in_format() {
        let gemma = server_args(&InferenceConfig { builtin_chat_format: Some("gemma".into()), ..InferenceConfig::default() }, 3888);
        assert!(gemma.contains(&"--no-jinja".to_string()));
        assert!(gemma.windows(2).any(|pair| pair[0] == "--chat-template" && pair[1] == "gemma"));
        let chatml = server_args(&InferenceConfig { builtin_chat_format: Some("chatml".into()), ..InferenceConfig::default() }, 3888);
        assert!(!chatml.contains(&"--no-jinja".to_string()), "ChatML is the runtime's own fallback");
        let own = server_args(&InferenceConfig::default(), 3888);
        assert!(!own.contains(&"--chat-template".to_string()) && !own.contains(&"--no-jinja".to_string()));
    }

    #[test]
    fn built_in_chat_formats_are_read_from_the_runtime_help() {
        // The pinned runtime's layout: the list wraps over indented lines and
        // ends at the option's environment variable.
        let help = "--chat-template JINJA_TEMPLATE          set custom jinja chat template\n\
                                        list of built-in templates:\n\
                                        bailing, chatml,\n\
                                        command-r, gemma, gpt-oss, hunyuan-dense, hunyuan-moe, llama4,\n\
                                        seed_oss, zephyr\n\
                                        (env: LLAMA_ARG_CHAT_TEMPLATE)\n\
--chat-template-file JINJA_TEMPLATE_FILE\n";
        let listed = listed_builtin_chat_templates(help);
        assert_eq!(listed.first().map(String::as_str), Some("bailing"));
        assert_eq!(listed.last().map(String::as_str), Some("zephyr"));
        for architecture in ["gemma3", "qwen3", "llama4", "command-r", "gpt-oss", "hunyuan-moe", "hunyuan-dense", "seed_oss"] {
            let format = crate::models::builtin_chat_format(architecture).unwrap();
            assert!(listed.iter().any(|name| name == format), "{architecture} -> {format}");
        }
        assert!(listed_builtin_chat_templates("no list here").is_empty());
    }

    #[test]
    fn a_draft_head_is_passed_with_its_draft_length() {
        let head = server_args(
            &InferenceConfig {
                speculative: crate::inference::DRAFT_HEAD_SPECULATIVE.into(),
                spec_draft_n_max: Some(3),
                ..InferenceConfig::default()
            },
            3888,
        );
        assert!(head.windows(2).any(|pair| pair[0] == "--spec-type" && pair[1] == "draft-mtp,ngram-simple"));
        assert!(head.windows(2).any(|pair| pair[0] == "--spec-draft-n-max" && pair[1] == "3"));
        let off = server_args(&InferenceConfig { speculative: "none".into(), spec_draft_n_max: Some(3), ..InferenceConfig::default() }, 3888);
        assert!(!off.contains(&"--spec-draft-n-max".to_string()), "no drafting, no draft length");
        let ngram = server_args(&InferenceConfig::default(), 3888);
        assert!(!ngram.contains(&"--spec-draft-n-max".to_string()));
        assert!(!ngram.contains(&"--spec-ngram-simple-size-m".to_string()), "the runtime's default length unless chosen");
        let split = server_args(&InferenceConfig { spec_ngram_length: Some(24), ..InferenceConfig::default() }, 3888);
        assert!(split.windows(2).any(|pair| pair[0] == "--spec-ngram-simple-size-m" && pair[1] == "24"));
        assert!(split.windows(2).any(|pair| pair[0] == "--spec-ngram-simple-size-n" && pair[1] == "12"));
        let off = server_args(&InferenceConfig { speculative: "none".into(), spec_ngram_length: Some(24), ..InferenceConfig::default() }, 3888);
        assert!(!off.contains(&"--spec-ngram-simple-size-m".to_string()), "no drafting, no length");
    }

    #[test]
    fn args_carry_the_micro_batch_and_keep_the_batch_at_least_as_large() {
        let args_for = |n_batch: u32, micro_batch: Option<u32>| {
            server_args(&InferenceConfig { n_batch, micro_batch, ..InferenceConfig::default() }, 3888)
        };
        let value = |args: &[String], key: &str| args.windows(2).find(|pair| pair[0] == key).map(|pair| pair[1].clone());

        let automatic = args_for(0, Some(1024));
        assert_eq!(value(&automatic[..], "--ubatch-size").as_deref(), Some("1024"));
        assert_eq!(value(&automatic[..], "--batch-size"), None, "the runtime's default logical batch (2,048) already holds it");

        let above_default = args_for(0, Some(4096));
        assert_eq!(value(&above_default[..], "--batch-size").as_deref(), Some("4096"), "llama.cpp would cap the micro-batch at 2,048");
        assert_eq!(value(&above_default[..], "--ubatch-size").as_deref(), Some("4096"));

        let small_batch = args_for(512, Some(1024));
        assert_eq!(value(&small_batch[..], "--batch-size").as_deref(), Some("1024"), "raised to the micro-batch");
        let large_batch = args_for(4096, Some(1024));
        assert_eq!(value(&large_batch[..], "--batch-size").as_deref(), Some("4096"), "a larger batch is kept");

        let unset = args_for(512, None);
        assert_eq!(value(&unset[..], "--batch-size").as_deref(), Some("512"));
        assert!(!unset.contains(&"--ubatch-size".to_string()), "no choice leaves llama-server's micro-batch");
    }

    #[test]
    fn args_load_without_mmap_only_when_chosen() {
        let mapped = server_args(&InferenceConfig::default(), 3888);
        assert!(!mapped.contains(&"--load-mode".to_string()));
        let pinned = server_args(&InferenceConfig { load_without_mmap: true, ..InferenceConfig::default() }, 3888);
        assert!(pinned.windows(2).any(|pair| pair[0] == "--load-mode" && pair[1] == "none"));
        assert!(!pinned.contains(&"--no-mmap".to_string()), "deprecated in the pinned runtime");
    }

    #[test]
    fn sse_frame_splitter_handles_lf_crlf_and_partial_frames() {
        let mut frames = SseFrames::new();
        frames.push(b"data: a\n\ndata: b\r\n\r\ndata: ");
        assert_eq!(frames.next_frame().unwrap(), b"data: a\n\n");
        assert_eq!(frames.next_frame().unwrap(), b"data: b\r\n\r\n");
        assert!(frames.next_frame().is_none());
        frames.mark_scanned();
        frames.push(b"c\n");
        assert!(frames.next_frame().is_none());
        frames.mark_scanned();
        frames.push(b"\n");
        assert_eq!(frames.next_frame().unwrap(), b"data: c\n\n");
        assert!(frames.buf.is_empty());
    }

    #[tokio::test]
    async fn worker_logs_are_drained_and_diagnostic_tail_is_bounded() {
        use tokio::io::AsyncWriteExt;
        let (reader, mut writer) = tokio::io::duplex(1024);
        let tail = Arc::new(Mutex::new(Vec::new()));
        let task = drain_worker_log(reader, tail.clone());
        let data = vec![b'x'; 128 * 1024];
        tokio::time::timeout(Duration::from_secs(2), writer.write_all(&data))
            .await
            .unwrap()
            .unwrap();
        writer.write_all(b"LAST LOG").await.unwrap();
        drop(writer);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let bytes = tail.lock().unwrap();
        assert_eq!(bytes.len(), 8192);
        assert!(bytes.ends_with(b"LAST LOG"));
    }

    #[tokio::test]
    async fn exited_worker_fails_health_immediately() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd.exe");
            command.args(["/C", "exit", "7"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 7"]);
            command
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.wait().await.unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_health(&mut child, "http://127.0.0.1:1", Duration::from_secs(90)),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("exited before becoming ready"));
    }

    #[tokio::test]
    async fn occupied_port_never_uses_another_workers_health() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = InferenceConfig {
            model_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            ..InferenceConfig::default()
        };
        // The occupied port must be rejected before even trying the binary.
        let result = RunningSidecar::spawn(
            Path::new("intentionally-missing-worker"),
            cfg,
            listener.local_addr().unwrap().port(),
        )
        .await;
        assert!(
            matches!(result, Err(error) if error.to_string().contains("already in use or unavailable"))
        );
    }

    #[test]
    fn the_runtime_is_found_at_the_installation_root_wherever_the_models_are() {
        std::env::remove_var("COMPANION_LLAMA_SERVER_BIN");
        let root = std::env::temp_dir().join(format!("companion-root-{}", uuid::Uuid::new_v4()));
        let bin = runtime_dir(&root);
        std::fs::create_dir_all(&bin).unwrap();
        let exe = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        std::fs::write(bin.join(exe), b"").unwrap();
        let found = SidecarBinary::detect(&bin).unwrap();
        assert_eq!(found.0, bin.join(exe));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_binary_error_is_actionable() {
        std::env::remove_var("COMPANION_LLAMA_SERVER_BIN");
        // Point PATH at an empty temp dir so detection deterministically fails.
        let empty = std::env::temp_dir().join(format!("companion-nobin-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &empty);
        let err = SidecarBinary::detect(&runtime_dir(Path::new("/nonexistent-install"))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("llama-server"), "{msg}");
        assert!(msg.contains("COMPANION_LLAMA_SERVER_BIN"), "{msg}");
        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[tokio::test]
    async fn client_chat_parses_openai_shape() {
        // Mock sidecar speaking the OpenAI-compatible dialect.
        let app = axum::Router::new()
            .route(
                "/v1/chat/completions",
                axum::routing::post(|| async {
                    axum::Json(serde_json::json!({
                        "choices": [{"message": {"content": "hello there"}}],
                        "usage": {"prompt_tokens": 5, "completion_tokens": 2}
                    }))
                }),
            )
            .route(
                "/tokenize",
                axum::routing::post(|| async {
                    axum::Json(serde_json::json!({"tokens": [1, 2, 3]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let c = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let (text, m) = c.chat("hi", 10, &InferenceConfig::default()).await.unwrap();
        assert_eq!(text, "hello there");
        assert_eq!(m.prompt_tokens, 5);
        assert_eq!(m.generated_tokens, 2);
        assert_eq!(c.tokenize("hi").await.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn client_surfaces_http_errors() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                (axum::http::StatusCode::BAD_REQUEST, "context overflow")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let c = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let err = c
            .chat("hi", 10, &InferenceConfig::default())
            .await
            .unwrap_err();
        // A plain-text 400 is a rejected request, typed, with the server's words.
        assert!(matches!(err.sidecar(), Some(SidecarFailure::BadRequest(detail)) if detail == "context overflow"), "{err}");
    }

    #[tokio::test]
    async fn completion_critic_disables_thinking_without_changing_normal_chat() {
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = bodies.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(body);
                    axum::Json(serde_json::json!({
                        "choices": [{"message": {"content": "COMPLETE"}}],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 2}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let turns = [ChatTurn::text("user", "Verify this result")];
        let cfg = InferenceConfig::default();
        let (text, metrics) = client
            .chat_turns_without_reasoning(&turns, 220, &cfg)
            .await
            .unwrap();
        assert_eq!(text, "COMPLETE");
        assert_eq!(metrics.generated_tokens, 2);
        client.chat_turns(&turns, 1024, &cfg).await.unwrap();
        let captured = bodies.lock().unwrap();
        assert_eq!(
            captured[0]["chat_template_kwargs"]["enable_thinking"],
            false
        );
        assert_eq!(captured[0]["max_tokens"], 220);
        assert!(captured[1].get("chat_template_kwargs").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn agent_completion_preserves_exhaustion_metadata_without_native_reasoning_text() {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = bodies.clone();
        let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let mut requests = captured.lock().unwrap();
                    requests.push(body);
                    let reply = match requests.len() {
                        1 => serde_json::json!({
                            "choices": [{"message": {"content": "", "reasoning_content": "PRIVATE SYNTHETIC REASONING"}, "finish_reason": "length"}],
                            "usage": {"prompt_tokens": 73, "completion_tokens": 2048, "completion_tokens_details": {"reasoning_tokens":2048}}
                        }),
                        2 => serde_json::json!({
                            "choices": [{"message": {"content": "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"}}\n```"}, "finish_reason": "length"}],
                            "usage": {"prompt_tokens": 74, "completion_tokens": 4096}
                        }),
                        _ => serde_json::json!({"choices":[{"message":{"content":"Normal chat"},"finish_reason":"stop"}]}),
                    };
                    axum::Json(reply)
                }
            }
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let turns = [ChatTurn::text("user", "Inspect the fixture")];
        let cfg = InferenceConfig::default();
        let first = client
            .agent_chat_turns(&turns, 2048, &cfg, false, false)
            .await
            .unwrap();
        assert!(first.text.is_empty());
        assert!(first.is_truncated());
        assert!(first.reasoning_present);
        assert_eq!(first.reasoning_tokens, Some(2048));
        assert_eq!(first.metrics.prompt_tokens, 73);
        assert_eq!(first.metrics.generated_tokens, 2048);
        assert!(!first.native_tool_calls_present);
        assert!(!serde_json::to_string(&first)
            .unwrap()
            .contains("PRIVATE SYNTHETIC REASONING"));
        let retry = client
            .agent_chat_turns(&turns, 4096, &cfg, true, false)
            .await
            .unwrap();
        assert!(
            retry.is_truncated(),
            "even parseable tool text must retain the provider's truncation signal"
        );
        assert!(crate::agent_runner::parse_tool_block(&retry.text).is_some());
        assert!(!retry.reasoning_present);
        assert!(retry.reasoning_tokens.is_none());
        client.chat_turns(&turns, 512, &cfg).await.unwrap();
        let requests = bodies.lock().unwrap();
        assert_eq!(requests.len(), 3, "client never silently retries");
        assert_eq!(requests[0]["max_tokens"], 2048);
        assert!(requests[0].get("chat_template_kwargs").is_none());
        assert_eq!(requests[1]["max_tokens"], 4096);
        assert_eq!(
            requests[1]["chat_template_kwargs"]["enable_thinking"],
            false
        );
        assert!(
            requests[2].get("chat_template_kwargs").is_none(),
            "agent recovery must not alter Chat"
        );
        server.abort();
    }

    #[tokio::test]
    async fn agent_completion_reports_unsupported_native_tool_calls() {
        let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(|| async {
            axum::Json(serde_json::json!({
                "choices": [{"message": {"content": null, "tool_calls": [{"id":"one","type":"function","function":{"name":"read_file","arguments":"{}"}}]}, "finish_reason":"tool_calls"}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 8}
            }))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = SidecarClient::new(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .agent_chat_turns(
                &[ChatTurn::text("user", "hi")],
                2048,
                &InferenceConfig::default(),
                false,
                false,
            )
            .await
            .unwrap();
        assert!(response.native_tool_calls_present);
        assert!(response.text.is_empty());
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!response.is_truncated());
        assert!(!response.reasoning_present);
        server.abort();
    }

    #[tokio::test]
    async fn structured_agent_fallback_is_opt_in_and_preserves_counted_transcript() {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = bodies.clone();
        let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(body);
                    axum::Json(serde_json::json!({
                        "choices": [{"message": {
                            "content": "{\"kind\":\"tool\",\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"},\"answer\":\"\"}",
                            "reasoning_content": "PRIVATE STRUCTURED TEST REASONING"
                        }, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 90, "completion_tokens": 35,
                            "completion_tokens_details": {"reasoning_tokens": 3}}
                    }))
                }
            }
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let turns = [
            ChatTurn::text("system", "Inspect only this fixture."),
            ChatTurn::text("user", "Read README.md"),
            ChatTurn::text(
                "user",
                "Caller-supplied JSON format instruction, already counted in its context.",
            ),
        ];
        let before = serde_json::to_value(&turns).unwrap();
        let cfg = InferenceConfig {
            temperature: 0.25,
            top_p: 0.85,
            top_k: 32,
            repeat_penalty: 1.05,
            ..InferenceConfig::default()
        };
        let completion = client
            .agent_chat_turns(&turns, 4096, &cfg, true, true)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&turns).unwrap(),
            before,
            "never mutate the caller's transcript"
        );
        assert_eq!(completion.finish_reason.as_deref(), Some("stop"));
        assert!(
            completion.reasoning_present,
            "metadata remains honest if the runtime ignores a thinking override"
        );
        assert_eq!(completion.reasoning_tokens, Some(3));
        assert!(!serde_json::to_string(&completion)
            .unwrap()
            .contains("PRIVATE STRUCTURED TEST REASONING"));
        let plain_turns = [ChatTurn::text("user", "Normal request")];
        client
            .agent_chat_turns(&plain_turns, 2048, &cfg, false, false)
            .await
            .unwrap();
        client.chat_turns(&plain_turns, 512, &cfg).await.unwrap();
        client
            .chat_turns_without_reasoning(&plain_turns, 220, &cfg)
            .await
            .unwrap();
        let requests = bodies.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[0]["messages"], before,
            "the helper must not add an uncounted instruction turn"
        );
        assert_eq!(requests[0]["max_tokens"], 4096);
        assert_eq!(
            requests[0]["temperature"],
            serde_json::json!(cfg.temperature)
        );
        assert_eq!(requests[0]["top_p"], serde_json::json!(cfg.top_p));
        assert_eq!(requests[0]["top_k"], cfg.top_k);
        assert_eq!(
            requests[0]["repeat_penalty"],
            serde_json::json!(cfg.repeat_penalty)
        );
        assert_eq!(
            requests[0]["chat_template_kwargs"]["enable_thinking"],
            false
        );
        assert_eq!(
            requests[0]["response_format"],
            SidecarClient::structured_action_schema(),
            "the per-tool envelope"
        );
        for request in &requests[1..] {
            assert!(
                request.get("response_format").is_none(),
                "schema must not leak to ordinary Agent, Chat or critic requests"
            );
            assert_eq!(request["messages"].as_array().unwrap().len(), 1);
        }
        assert!(requests[1].get("chat_template_kwargs").is_none());
        assert!(requests[2].get("chat_template_kwargs").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn client_stream_forwards_deltas() {
        use axum::response::sse::{Event, KeepAlive, Sse};
        // Mock sidecar emitting OpenAI SSE deltas then [DONE].
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                let frames = vec![
                    Ok::<_, std::convert::Infallible>(
                        Event::default().data(r#"{"choices":[{"delta":{"content":"Hello "}}]}"#),
                    ),
                    Ok::<_, std::convert::Infallible>(
                        Event::default().data(r#"{"choices":[{"delta":{"content":"world"}}]}"#),
                    ),
                    Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]")),
                ];
                Sse::new(futures::stream::iter(frames)).keep_alive(KeepAlive::default())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let c = SidecarClient::new(format!("http://127.0.0.1:{port}")).unwrap();
        let toks = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let toks_cb = toks.clone();
        let m = c
            .chat_stream(
                "hi",
                10,
                &InferenceConfig::default(),
                move |t| toks_cb.lock().unwrap().push(t),
                || false,
            )
            .await
            .unwrap();
        assert_eq!(toks.lock().unwrap().join(""), "Hello world");
        // Without a tokenizer endpoint, chunk count is not token count.
        assert_eq!(m.generated_tokens, 3);
        let timing = m.timing.unwrap();
        assert!(timing.estimated);
        assert_eq!(timing.token_basis, "character_estimate");
        assert!(timing.first_visible_ms.is_some());
        assert!(timing.thinking_ms.is_none());
    }

    #[test]
    fn visible_clock_excludes_preparation_reasoning_and_tail_wait() {
        let mut clock = VisibleOutputClock::default();
        clock.reasoning(500);
        clock.reasoning(700);
        clock.visible(1500);
        clock.visible(1700);
        clock.reasoning(1900);
        clock.visible(2900);
        clock.visible(3200);
        let timing = clock.finish(4000);
        assert_eq!(timing.first_visible_ms, Some(1500));
        assert_eq!(timing.output_ms, 500);
        assert_eq!(timing.thinking_ms, Some(2000));
        assert_eq!(timing.total_ms, 4000);
    }

    #[test]
    fn visible_clock_does_not_invent_rate_or_thinking_from_silence() {
        let empty = VisibleOutputClock::default().finish(5000);
        assert!(empty.first_visible_ms.is_none());
        assert!(empty.thinking_ms.is_none());
        let mut single = VisibleOutputClock::default();
        single.visible(4500);
        let mut timing = single.finish(5000);
        timing.output_tokens = 10;
        timing.update_rate();
        assert_eq!(timing.first_visible_ms, Some(4500));
        assert_eq!(timing.output_ms, 0);
        assert!(
            timing.output_tps.is_none(),
            "buffered single-delta output has no emission interval"
        );
    }

    #[tokio::test]
    async fn observed_stream_counts_visible_text_only_and_preserves_split_unicode() {
        use futures::StreamExt;
        let empty =
            b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\r\n\r\n"
                .to_vec();
        let reasoning = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"PRIVATE TEST REASONING\",\"content\":\"\"}}]}\n\n".to_vec();
        let unicode = "data: {\"choices\":[{\"delta\":{\"content\":\"界\"}}]}\r\n\r\n"
            .as_bytes()
            .to_vec();
        let split = unicode.iter().position(|byte| *byte == 0xe7).unwrap() + 1;
        let chunks = vec![
            empty, reasoning, unicode[..split].to_vec(), unicode[split..].to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":13,\"completion_tokens\":500}}\n\ndata: [DONE]\n\n".to_vec(),
        ];
        let app = axum::Router::new()
            .route(
                "/v1/chat/completions",
                axum::routing::post(move || {
                    let chunks = chunks.clone();
                    async move {
                        let stream = futures::stream::iter(chunks).then(|chunk| async move {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            Ok::<_, std::convert::Infallible>(chunk)
                        });
                        axum::http::Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(axum::body::Body::from_stream(stream))
                            .unwrap()
                    }
                }),
            )
            .route(
                "/tokenize",
                axum::routing::post(
                    |axum::Json(body): axum::Json<serde_json::Value>| async move {
                        assert_eq!(body["content"], "界!");
                        assert_eq!(body["add_special"], false);
                        axum::Json(serde_json::json!({"tokens":[1, 2]}))
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let visible = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let phases = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let visible_cb = visible.clone();
        let phase_cb = phases.clone();
        let metrics = SidecarClient::new(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .chat_turns_stream_observed(
                &[ChatTurn::text("user", "hi")],
                1024,
                &InferenceConfig::default(),
                move |text| visible_cb.lock().unwrap().push_str(&text),
                move |phase| phase_cb.lock().unwrap().push(phase),
                || false,
            )
            .await
            .unwrap();
        assert_eq!(*visible.lock().unwrap(), "界!");
        assert_eq!(
            *phases.lock().unwrap(),
            vec!["processing", "thinking", "responding"]
        );
        assert_eq!(
            metrics.generated_tokens, 500,
            "provider total usage is separate from visible output"
        );
        assert_eq!(metrics.prompt_tokens, 13);
        let timing = metrics.timing.unwrap();
        assert_eq!(timing.output_tokens, 2);
        assert!(!timing.estimated);
        assert_eq!(timing.token_basis, "tokenizer");
        assert!(
            timing.first_visible_ms.unwrap() >= 75,
            "empty/hidden deltas must not start the output clock"
        );
        assert!(timing.thinking_ms.unwrap() >= 25);
        assert!(timing.output_ms > 0);
        assert!(timing.output_ms < timing.first_visible_ms.unwrap());
        assert_eq!(
            timing.output_tps.unwrap(),
            (2000.0 / timing.output_ms as f64 * 10.0).round() / 10.0
        );
        server.abort();
    }
}
