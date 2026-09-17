//! Inference abstraction (§100).
//!
//! The rest of the application talks to `IInferenceEngine`, never directly
//! to llama.cpp. `LlamaCppEngine` (Stage 3) will implement this trait via
//! llama.cpp bindings. `StubEngine` provides a deterministic stand-in so the
//! API, agent loop, and UI can be developed and tested without a GGUF file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror_stub::InferenceError;

pub mod thiserror_stub {
    #[derive(Debug)]
    pub enum InferenceError {
        ModelNotLoaded,
        ModelFileMissing(String),
        InsufficientMemory { required_gb: f64, available_gb: f64 },
        ContextOverflow { used: u32, limit: u32 },
        Generation(String),
        /// The model server received the request and refused it or could not
        /// finish it, classified so callers recover by kind instead of by
        /// reading message text.
        Sidecar(SidecarFailure),
    }

    impl InferenceError {
        pub fn sidecar(&self) -> Option<&SidecarFailure> {
            match self {
                Self::Sidecar(failure) => Some(failure),
                _ => None,
            }
        }
    }

    /// Why a model-server request failed. Classified from the HTTP status and
    /// llama-server's own error object (`{"error": {"code", "message", "type",
    /// ...}}`, e.g. `exceed_context_size_error` with `n_prompt_tokens` and
    /// `n_ctx`), or from the transport when no response arrived.
    #[derive(Debug, Clone, PartialEq)]
    pub enum SidecarFailure {
        /// The prompt does not fit the loaded context. Nothing was generated.
        /// Counts are 0 when the server did not report them.
        ContextExceeded { prompt_tokens: u32, context: u32 },
        /// No usable response: connection refused or reset, or the server is
        /// loading or restarting.
        Unavailable(String),
        /// The request timed out.
        Timeout,
        /// The reply stream ended before the server finished it.
        Truncated,
        /// The server rejected the request as invalid.
        BadRequest(String),
        /// The server failed while handling the request.
        Server(String),
    }

    impl SidecarFailure {
        /// Worth repeating unchanged after a short wait: the request itself
        /// was not the problem.
        pub fn is_transient(&self) -> bool {
            matches!(self, Self::Unavailable(_) | Self::Timeout | Self::Truncated)
        }
    }

    impl std::fmt::Display for SidecarFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::ContextExceeded { prompt_tokens, context } if *prompt_tokens > 0 && *context > 0 => write!(
                    f,
                    "the request ({prompt_tokens} tokens) does not fit the model's {context}-token context"
                ),
                Self::ContextExceeded { .. } => write!(f, "the request does not fit the model's context"),
                Self::Unavailable(detail) if detail.is_empty() => write!(f, "the model server did not respond"),
                Self::Unavailable(detail) => write!(f, "the model server did not respond ({detail})"),
                Self::Timeout => write!(f, "the model server did not answer in time"),
                Self::Truncated => write!(f, "the model server stopped the reply before finishing it"),
                Self::BadRequest(detail) => write!(f, "the model server rejected the request: {detail}"),
                Self::Server(detail) => write!(f, "the model server failed while answering: {detail}"),
            }
        }
    }

    impl std::fmt::Display for InferenceError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                // §52: user-friendly messages, never raw GGML asserts.
                Self::ModelNotLoaded => {
                    write!(
                        f,
                        "No model is loaded. Select a model and press Load first."
                    )
                }
                Self::ModelFileMissing(p) => {
                    write!(
                        f,
                        "Model file not found: {p}. Check the model directory in Settings."
                    )
                }
                Self::InsufficientMemory {
                    required_gb,
                    available_gb,
                } => write!(
                    f,
                    "The model needs ~{required_gb:.1} GB but only {available_gb:.1} GB is \
                     available. Try Hybrid CPU/GPU mode, fewer GPU layers, a smaller \
                     quantization, or a smaller model."
                ),
                Self::ContextOverflow { used, limit } => write!(
                    f,
                    "Conversation ({used} tokens) exceeds the {limit}-token context. \
                     Summarize older messages or raise the context size in Settings."
                ),
                Self::Generation(msg) => write!(f, "Generation failed: {msg}"),
                Self::Sidecar(failure) => {
                    let text = failure.to_string();
                    let mut chars = text.chars();
                    match chars.next() {
                        Some(first) => write!(f, "{}{}", first.to_uppercase(), chars.as_str()),
                        None => Ok(()),
                    }
                }
            }
        }
    }

    impl std::error::Error for InferenceError {}
}

/// Whether the loaded runtime reported a template capability. `Unknown` when
/// it did not say: callers claim less for an unknown capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    Yes,
    No,
    #[default]
    Unknown,
}

/// What llama-server reports the model's chat template can render
/// (`GET /props` → `chat_template_caps`), read once per load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TemplateCaps {
    pub tools: Support,
    pub tool_calls: Support,
    pub system_role: Support,
    pub parallel_tool_calls: Support,
    pub preserve_reasoning: Support,
}

impl TemplateCaps {
    pub fn from_props(props: &serde_json::Value) -> Option<Self> {
        let caps = props.get("chat_template_caps")?.as_object()?;
        let read = |key: &str| match caps.get(key).and_then(|value| value.as_bool()) {
            Some(true) => Support::Yes,
            Some(false) => Support::No,
            None => Support::Unknown,
        };
        Some(Self {
            tools: read("supports_tools"),
            tool_calls: read("supports_tool_calls"),
            system_role: read("supports_system_role"),
            parallel_tool_calls: read("supports_parallel_tool_calls"),
            preserve_reasoning: read("supports_preserve_reasoning"),
        })
    }

    /// The template renders a tool list or tool calls.
    pub fn tools_supported(&self) -> Support {
        match (self.tools, self.tool_calls) {
            (Support::Yes, _) | (_, Support::Yes) => Support::Yes,
            (Support::No, Support::No) => Support::No,
            _ => Support::Unknown,
        }
    }
}

/// Restrictions a chat template places on the message list. Both defaults
/// mean "no restriction": a template is marked strict only when its own source
/// says so, never guessed from the model's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTemplateShape {
    /// The template renders a system turn. False means it raises on one.
    pub system_role: bool,
    /// The template requires user and assistant turns to alternate strictly,
    /// beginning with user.
    pub strict_alternation: bool,
}

impl Default for ChatTemplateShape {
    fn default() -> Self {
        Self {
            system_role: true,
            strict_alternation: false,
        }
    }
}

impl ChatTemplateShape {
    pub fn is_restricted(&self) -> bool {
        !self.system_role || self.strict_alternation
    }
}

/// Sampling / loading parameters (§49 Inference + §13 advanced panel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub model_path: PathBuf,
    pub projector_path: Option<PathBuf>,
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_threads: u32,
    /// Prompt-processing threads (`--threads-batch`); 0 means the same as
    /// `n_threads`. Separate because prompts come in short bursts that can use
    /// every core, while generation runs long and can leave the machine room.
    #[serde(default)]
    pub n_threads_batch: u32,
    /// `--poll` (0-100): how much worker threads spin-wait between operations.
    /// None leaves the runtime default.
    #[serde(default)]
    pub poll: Option<u8>,
    /// `--prio`: -1 lets other applications take the CPU first.
    #[serde(default)]
    pub priority: Option<i8>,
    /// `--fit-target` in MiB: VRAM llama-server's load-time fit leaves free.
    /// None leaves the server default (1024).
    #[serde(default)]
    pub fit_target_mib: Option<u32>,
    /// `--cache-ram` in MiB: llama-server's RAM prompt cache for switching
    /// between conversations. None leaves the server default (8192 MiB).
    #[serde(default)]
    pub cache_ram_mib: Option<u32>,
    /// `--ubatch-size`: tokens computed per step while reading a prompt. None
    /// leaves llama-server's default (512). Chosen in automatic modes only, by
    /// the owner's speed rule (`runtime_fit::default_micro_batch` or the
    /// model's calibration); a larger micro-batch reads prompts faster but its
    /// compute buffer takes VRAM the model could use.
    #[serde(default)]
    pub micro_batch: Option<u32>,
    /// `--load-mode none`: read the weights into RAM instead of mapping the
    /// model file. Set in automatic modes for placements that keep weights in
    /// RAM while using the GPU, when RAM has room for the copy
    /// (`runtime_fit::load_without_mmap`).
    #[serde(default)]
    pub load_without_mmap: bool,
    pub n_gpu_layers: i32, // -1 = auto, 0 = CPU, 999 = full offload
    pub flash_attn: bool,
    #[serde(default = "default_true")]
    pub flash_attn_auto: bool,
    pub kv_cache_gpu: bool,
    #[serde(default = "default_cache_type")]
    pub kv_cache_type_k: String,
    #[serde(default = "default_cache_type")]
    pub kv_cache_type_v: String,
    #[serde(default)]
    pub runtime_policy: Option<ResolvedRuntimePolicy>,
    /// Minimum KV chunk (tokens) llama-server may shift-reuse when a prompt
    /// diverges from the cached one. 0 disables chunk reuse (only the exact
    /// common prefix is reused). Agent loops prune and edit the middle of the
    /// transcript, so chunk reuse avoids re-prefilling the unchanged tail.
    #[serde(default = "default_cache_reuse")]
    pub cache_reuse: u32,
    /// Self-speculative decoding mode passed as `--spec-type` ("none" omits
    /// the flag). N-gram drafting needs no draft model: it proposes tokens that
    /// already occur in the context, which is exactly what code edits, file
    /// rewrites and repeated tool envelopes produce. Lossless by construction.
    #[serde(default = "default_speculative")]
    pub speculative: String,
    /// `--spec-draft-n-max`: tokens a draft model or built-in draft head
    /// proposes per step. None leaves the runtime's default; n-gram drafting
    /// ignores it (its length is `--spec-ngram-simple-size-m`).
    #[serde(default)]
    pub spec_draft_n_max: Option<u32>,
    /// `--spec-ngram-simple-size-m`: tokens n-gram drafting proposes at once.
    /// None leaves the runtime's default (48). Chosen by the load path from
    /// the placement (`runtime_fit::ngram_draft_length`), automatic modes only.
    #[serde(default)]
    pub spec_ngram_length: Option<u32>,
    /// llama.cpp's built-in chat format (`--no-jinja --chat-template NAME`)
    /// for a model file without a chat template; None uses the file's own
    /// template. Set by the load path from `models::builtin_chat_format`.
    #[serde(default)]
    pub builtin_chat_format: Option<String>,
    /// What the model's own chat template accepts. Gemma 2 and some Llama 2
    /// derivatives refuse a system role outright and raise unless user and
    /// assistant turns strictly alternate, so the message list that works
    /// everywhere else returns 400 for them. Read from the GGUF template at
    /// load; the default is permissive, as almost every modern template is.
    #[serde(default)]
    pub chat_template: ChatTemplateShape,
    /// Capabilities the runtime reported for the loaded template; None until
    /// read (older runtimes do not report them).
    #[serde(default)]
    pub template_caps: Option<TemplateCaps>,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub repeat_penalty: f32,
    pub seed: Option<u64>,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/model.gguf"),
            projector_path: None,
            n_ctx: 32768,
            n_batch: 512,
            n_threads: 0,
            n_threads_batch: 0,
            poll: None,
            priority: None,
            cache_ram_mib: None,
            fit_target_mib: None,
            micro_batch: None,
            load_without_mmap: false,
            n_gpu_layers: -1,
            flash_attn: true,
            flash_attn_auto: true,
            kv_cache_gpu: true,
            kv_cache_type_k: default_cache_type(),
            kv_cache_type_v: default_cache_type(),
            runtime_policy: None,
            cache_reuse: default_cache_reuse(),
            speculative: default_speculative(),
            spec_draft_n_max: None,
            spec_ngram_length: None,
            builtin_chat_format: None,
            chat_template: ChatTemplateShape::default(),
            template_caps: None,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            repeat_penalty: 1.1,
            seed: None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_cache_type() -> String {
    "f16".into()
}
fn default_cache_reuse() -> u32 {
    256
}
/// Measured on this project's benchmark suite (docs/PERFORMANCE.md): ngram-simple
/// had no measurable cost on novel prose and a 12-15x gain on file rewrites.
pub fn default_speculative() -> String {
    "ngram-simple".into()
}

/// Drafting for a model whose GGUF carries a built-in next-token prediction
/// layer (`<arch>.nextn_predict_layers`): the draft head plus n-gram drafting.
/// Measured on a 27B model with one such layer (all layers on the GPU, 8K
/// context): prose 34.9 → 52.9 tok/s, file rewrites 35.3 → 88.3 tok/s against
/// no drafting (n-gram alone: 36.0 and 62.3), at +0.9 GB of VRAM.
pub const DRAFT_HEAD_SPECULATIVE: &str = "draft-mtp,ngram-simple";
/// Tokens the draft head proposes per step: 3 was fastest measured (1: 48.6
/// prose tok/s, 2: 52.4, 3: 53.7; the runtime's default is also 3).
pub const DRAFT_HEAD_TOKENS: u32 = 3;
/// VRAM the draft head's layer and context take at `DRAFT_HEAD_TOKENS` (+0.9 GB
/// measured). `llama-fit-params` does not include it, so the load path's
/// probes leave this much more free when the draft head is on.
pub const DRAFT_HEAD_RESERVE_MIB: u32 = 1024;

/// The drafting a load uses: off when the owner turned it off; the draft head
/// with n-gram drafting when the model has one; n-gram drafting otherwise.
pub fn speculative_for(model: Option<&crate::models::ModelMetadata>, turned_off: bool) -> (String, Option<u32>) {
    if turned_off {
        return ("none".into(), None);
    }
    match model.and_then(|model| model.draft_head_layers) {
        Some(layers) if layers > 0 => (DRAFT_HEAD_SPECULATIVE.into(), Some(DRAFT_HEAD_TOKENS)),
        _ => (default_speculative(), None),
    }
}

/// Engine-reported timings for one completion, straight from llama-server's
/// `timings` object. These are the numbers the runtime itself measured: prompt
/// (prefill) throughput, decode throughput, and how many prompt tokens were
/// served from the KV cache instead of being recomputed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EngineTimings {
    pub prompt_tokens: u32,
    pub prompt_ms: f64,
    pub prompt_tps: Option<f64>,
    pub predicted_tokens: u32,
    pub predicted_ms: f64,
    pub predicted_tps: Option<f64>,
    /// Prompt tokens reused from the slot's KV cache (not re-prefilled).
    pub cached_tokens: u32,
    /// Speculative decoding statistics when the runtime drafted tokens.
    #[serde(default)]
    pub draft_tokens: Option<u32>,
    #[serde(default)]
    pub draft_accepted: Option<u32>,
}

impl EngineTimings {
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let timings = value.as_object()?;
        let number = |key: &str| timings.get(key).and_then(|v| v.as_f64());
        let count = |key: &str| number(key).map(|v| v.max(0.0).min(u32::MAX as f64) as u32);
        let predicted_tokens = count("predicted_n")?;
        let rate = |tokens: u32, ms: f64| {
            (tokens > 0 && ms > 0.0).then(|| (tokens as f64 * 1000.0 / ms * 10.0).round() / 10.0)
        };
        let prompt_tokens = count("prompt_n").unwrap_or(0);
        let prompt_ms = number("prompt_ms").unwrap_or(0.0);
        let predicted_ms = number("predicted_ms").unwrap_or(0.0);
        Some(Self {
            prompt_tokens,
            prompt_ms,
            prompt_tps: rate(prompt_tokens, prompt_ms),
            predicted_tokens,
            predicted_ms,
            predicted_tps: rate(predicted_tokens, predicted_ms),
            cached_tokens: count("cache_n").unwrap_or(0),
            draft_tokens: count("draft_n"),
            draft_accepted: count("draft_n_accepted"),
        })
    }
}

/// Resolved launch policy, not a claim about measured device placement. The
/// native runtime constructs architecture-specific attention/recurrent state.
/// Weight quantization is deliberately not used to choose cache precision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRuntimePolicy {
    pub mode: String,
    pub architecture: String,
    pub weights_quantization: String,
    pub requested_context: u32,
    pub effective_context: u32,
    pub cache_type_k: String,
    pub cache_type_v: String,
    pub flash_attention: String,
    pub kv_offload: String,
    /// Zero means omit the worker flag and let its native default choose.
    pub threads: u32,
    pub gpu_layers: i32,
    /// Zero means the runtime's own default logical/physical batch (2048/512
    /// in the bundled build), which prefilled measurably faster than a forced
    /// 512 logical batch.
    pub batch_size: u32,
    #[serde(default = "default_cache_reuse")]
    pub cache_reuse: u32,
    #[serde(default = "default_speculative")]
    pub speculative: String,
    /// `--spec-draft-n-max` for the draft head; None otherwise.
    #[serde(default)]
    pub spec_draft_n_max: Option<u32>,
    /// Context ceiling applied if the load falls back to the CPU, sized from
    /// free system RAM (8192 when RAM is plentiful or unknown).
    #[serde(default = "default_cpu_context_cap")]
    pub cpu_context_cap: u32,
    /// Planned residency from measured memory: gpu | hybrid | oversubscribed
    /// | cpu | unknown. A plan, not a measurement of where layers landed.
    #[serde(default = "default_placement")]
    pub placement: String,
    pub cache_rebuild: String,
    pub notes: Vec<String>,
    /// Why the loaded window differs from the requested one, alone. The full
    /// note list is runtime detail for the settings page; the context meter
    /// needs only the sentence that explains the number being read there.
    #[serde(default)]
    pub context_note: Option<String>,
}

fn default_cpu_context_cap() -> u32 {
    8192
}

fn default_placement() -> String {
    "unknown".into()
}

pub fn resolve_runtime_policy(
    model: Option<&crate::models::ModelMetadata>,
    requested: &InferenceConfig,
    automatic: bool,
) -> ResolvedRuntimePolicy {
    resolve_runtime_policy_with(
        model,
        requested,
        automatic,
        &crate::settings::RuntimeSettings::default(),
    )
}

pub fn resolve_runtime_policy_with(
    model: Option<&crate::models::ModelMetadata>,
    requested: &InferenceConfig,
    automatic: bool,
    tuning: &crate::settings::RuntimeSettings,
) -> ResolvedRuntimePolicy {
    resolve_runtime_policy_fitted(model, requested, automatic, tuning, None)
}

/// Measured GPU memory at load time, in bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VramState {
    pub total_bytes: u64,
    /// Memory other processes (desktop, browser) already hold.
    pub used_bytes: u64,
}

/// KV-cache bytes per token for a cache type relative to f16.
fn cache_bytes_per_token(f16_bytes: u64, cache_type: &str) -> u64 {
    match cache_type {
        // q8_0: 8-bit values plus one f16 scale per 32 -> 8.5 bits/value.
        "q8_0" => f16_bytes * 17 / 32,
        _ => f16_bytes,
    }
}

/// Where the loaded model will live, decided from measured memory.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryPlan {
    pub context: u32,
    pub cache_type: String,
    /// "gpu" (fully resident), "hybrid" (weights split between GPU and RAM),
    /// "oversubscribed" (does not fit RAM + VRAM either).
    pub placement: &'static str,
    pub note: Option<String>,
}

/// Fit the context (and, if allowed, the cache precision) to the machine.
/// Spilling layers to the CPU costs 3-5x in decode speed (measured: a 14B
/// model at 32K f16 ran at 18 tok/s on a 12 GB card) while a smaller context
/// or an 8-bit cache costs almost nothing, so the order of preference is:
/// everything on the GPU; a smaller context or q8_0 cache on the GPU; and only
/// then a hybrid split, sized so the GPU holds as much of the weights as
/// possible and the remainder fits system RAM. None when the inputs are
/// unknown or there is no GPU (the CPU cap handles that case).
fn fit_to_memory(
    requested_context: u32,
    cache_type: &str,
    allow_q8: bool,
    model: &crate::models::ModelMetadata,
    vram: VramState,
    ram_available: Option<u64>,
) -> Option<MemoryPlan> {
    let weights = model.weights_bytes?;
    let kv_f16 = model.kv_bytes_per_token?;
    if vram.total_bytes == 0 {
        return None;
    }
    let gb = |bytes: u64| bytes as f64 / 1e9;
    // Room the runtime can actually use: what is free now, minus compute
    // buffers (grow with model width) and a safety margin for fragmentation.
    let free = vram.total_bytes.saturating_sub(vram.used_bytes);
    let compute = (weights / 12).max(768 * 1024 * 1024);
    // Measured on a 12 GB card with 1.7 GB held by other applications: a
    // 14B Q4 model (9.0 GB) ran fully on the GPU at 11,264 tokens with an
    // 8-bit cache (72 tok/s) and spilled at 12,288 (49 tok/s). The compute
    // reserve alone matches that boundary; this margin covers what other
    // applications add while the model loads (they varied by ~0.4 GB). A 5%
    // margin cost the same model more than half its usable window.
    let safety = (vram.total_bytes / 48).max(256 * 1024 * 1024);
    let gpu_budget = free.saturating_sub(compute).saturating_sub(safety);
    let kv_bytes = |context: u32, cache: &str| cache_bytes_per_token(kv_f16, cache) * context as u64;
    let fits_gpu = |context: u32, cache: &str| weights.saturating_add(kv_bytes(context, cache)) <= gpu_budget;
    if fits_gpu(requested_context, cache_type) {
        return Some(MemoryPlan {
            context: requested_context,
            cache_type: cache_type.to_string(),
            placement: "gpu",
            note: None,
        });
    }
    // The largest context a precision allows within `room` bytes, in
    // 1,024-token steps. Halving from the request (32K, 16K, 8K, 4K) turned a
    // few hundred megabytes of shortfall into half the window: a 14B model
    // that fits 7.7K tokens got 4K.
    // Below 4K an agent cannot hold its instructions and one action; a smaller
    // saved preference is honoured as the floor instead of being raised.
    let floor = requested_context.min(4096);
    let largest = |room: u64, cache: &str| -> Option<u32> {
        let per_token = cache_bytes_per_token(kv_f16, cache).max(1);
        let tokens = (room / per_token).min(u64::from(requested_context)) as u32;
        let stepped = if tokens >= requested_context {
            requested_context
        } else {
            tokens / 1024 * 1024
        };
        (stepped >= floor).then_some(stepped)
    };
    let mut precisions = vec![cache_type];
    if allow_q8 && cache_type != "q8_0" {
        precisions.push("q8_0");
    }
    let gpu_room = gpu_budget.checked_sub(weights);
    // Largest context first; at equal context prefer the requested precision.
    if let Some((context, cache)) = gpu_room.and_then(|room| {
        precisions
            .iter()
            .filter_map(|cache| largest(room, cache).map(|context| (context, *cache)))
            .max_by_key(|(context, cache)| (*context, u8::from(*cache == cache_type)))
    }) {
        let note = if cache != cache_type && context < requested_context {
            format!(
                "Context reduced from {requested_context} to {context} tokens and the KV cache stored as q8_0 (8-bit) so the whole model stays on the GPU: {:.1} GB of weights plus a {:.1} GB cache fit the {:.1} GB of free GPU memory; the f16 cache would have forced layers onto the CPU. The saved preference is unchanged; a smaller model or quantization allows a larger context.",
                gb(weights), gb(kv_bytes(context, cache)), gb(free)
            )
        } else if cache != cache_type {
            format!(
                "Context kept at {context} tokens by storing the KV cache as q8_0 (8-bit): {:.1} GB of weights plus a {:.1} GB cache fit the {:.1} GB of free GPU memory, where the f16 cache would have forced layers onto the CPU. Set KV cache precision to f16 in Settings to prefer a smaller f16 context instead.",
                gb(weights), gb(kv_bytes(context, cache)), gb(free)
            )
        } else {
            format!(
                "Context reduced from {requested_context} to {context} tokens so the whole model stays on the GPU: {:.1} GB of weights plus a {:.1} GB {cache} cache fit the {:.1} GB of free GPU memory. The saved preference is unchanged; a smaller model or quantization allows a larger context.",
                gb(weights), gb(kv_bytes(context, cache)), gb(free)
            )
        };
        return Some(MemoryPlan {
            context,
            cache_type: cache.to_string(),
            placement: "gpu",
            note: Some(note),
        });
    }
    // Near fit: the weights fit the budget and only the smallest cache does
    // not, by less than the safety margin. Take the smallest window and keep
    // the model on the GPU. The hybrid rule below would reserve a quarter of
    // the GPU for a large cache (21K tokens for a 14B model 70 MB short) and
    // push weights to the CPU, the slowest plan for a model that nearly fits.
    let small_cache = if allow_q8 { "q8_0" } else { cache_type };
    if weights <= gpu_budget
        && weights.saturating_add(kv_bytes(floor, small_cache)) <= gpu_budget.saturating_add(safety)
    {
        return Some(MemoryPlan {
            context: floor,
            cache_type: small_cache.to_string(),
            placement: "gpu",
            note: Some(format!(
                "Context reduced from {requested_context} to {floor} tokens with a {small_cache} KV cache: {:.1} GB of weights leave little of the {:.1} GB of free GPU memory, and a larger window would push layers onto the CPU. Closing other GPU-heavy applications before loading allows a larger context.",
                gb(weights),
                gb(free)
            )),
        });
    }
    // Hybrid: the weights alone overflow the GPU. Keep the cache small (at
    // most a quarter of the GPU budget, 8-bit when allowed) so the GPU holds
    // as many layers as possible, and check the remainder against RAM.
    let hybrid_cache = if allow_q8 { "q8_0" } else { cache_type };
    let kv_room = gpu_budget / 4;
    let context = largest(kv_room, hybrid_cache).unwrap_or(floor);
    let gpu_weights = gpu_budget.saturating_sub(kv_bytes(context, hybrid_cache));
    let cpu_weights = weights.saturating_sub(gpu_weights);
    let cpu_share = cpu_weights as f64 / weights.max(1) as f64;
    let ram_needed = cpu_weights.saturating_add((kv_bytes(context, hybrid_cache) as f64 * cpu_share) as u64);
    let ram_budget = ram_available.map(|ram| ram.saturating_sub(ram / 8).saturating_sub(compute / 2));
    let (placement, ram_note) = match ram_budget {
        Some(budget) if ram_needed > budget => (
            "oversubscribed",
            format!(" System RAM ({:.1} GB free) cannot hold the {:.1} GB that does not fit the GPU; loading may fail or page heavily. Use a smaller model or quantization.", gb(ram_available.unwrap_or(0)), gb(ram_needed)),
        ),
        Some(_) => ("hybrid", String::new()),
        None => ("hybrid", " Free system RAM was not measured.".into()),
    };
    Some(MemoryPlan {
        context,
        cache_type: hybrid_cache.to_string(),
        placement,
        note: Some(format!(
            "The weights ({:.1} GB) exceed the {:.1} GB of free GPU memory, so about {:.1} GB ({:.0}%) of the layers will run on the CPU and replies will be slower. Context {context} with a {hybrid_cache} cache keeps the cache under a quarter of GPU memory so the GPU holds as much of the model as possible.{ram_note}",
            gb(weights), gb(free), gb(cpu_weights), cpu_share * 100.0
        )),
    })
}

/// Largest context the CPU fallback may use: the comfort cap (decode slows as
/// a CPU-side cache fills) lowered to what system RAM can hold next to the
/// weights. Returns the cap and a note when RAM, not the cap, decided.
pub fn cpu_context_cap(
    model: Option<&crate::models::ModelMetadata>,
    ram_available_bytes: Option<u64>,
) -> (u32, Option<String>) {
    const COMFORT_CAP: u32 = 8192;
    let (Some(model), Some(ram)) = (model, ram_available_bytes) else {
        return (COMFORT_CAP, None);
    };
    let (Some(weights), Some(kv)) = (model.weights_bytes, model.kv_bytes_per_token) else {
        return (COMFORT_CAP, None);
    };
    // Keep an eighth of free RAM for the OS and page cache, plus compute buffers.
    let budget = ram
        .saturating_sub(ram / 8)
        .saturating_sub((weights / 12).max(512 * 1024 * 1024));
    let room = budget.saturating_sub(weights);
    if weights > budget || room < kv * 2048 {
        return (
            2048,
            Some(format!(
                "System RAM is tight for this model on CPU ({:.1} GB of weights, {:.1} GB free): context is limited to 2048 tokens and the OS may page. A smaller model or quantization would be far more responsive.",
                weights as f64 / 1e9,
                ram as f64 / 1e9
            )),
        );
    }
    let by_ram = (room / kv).min(u32::MAX as u64) as u32;
    let mut cap = COMFORT_CAP;
    while cap > 2048 && cap > by_ram {
        cap /= 2;
    }
    let note = (cap < COMFORT_CAP).then(|| {
        format!(
            "CPU context limited to {cap} tokens by free system RAM ({:.1} GB): the {:.1} GB of weights plus the cache must stay resident.",
            ram as f64 / 1e9,
            weights as f64 / 1e9
        )
    });
    (cap, note)
}

pub fn resolve_runtime_policy_fitted(
    model: Option<&crate::models::ModelMetadata>,
    requested: &InferenceConfig,
    automatic: bool,
    tuning: &crate::settings::RuntimeSettings,
    vram: Option<VramState>,
) -> ResolvedRuntimePolicy {
    resolve_runtime_policy_for_machine(model, requested, automatic, tuning, vram, None)
}

pub fn resolve_runtime_policy_for_machine(
    model: Option<&crate::models::ModelMetadata>,
    requested: &InferenceConfig,
    automatic: bool,
    tuning: &crate::settings::RuntimeSettings,
    vram: Option<VramState>,
    ram_available_bytes: Option<u64>,
) -> ResolvedRuntimePolicy {
    let requested_context = requested.n_ctx.max(1);
    let mut effective_context = model
        .map(|model| requested_context.min(model.context_length.max(1)))
        .unwrap_or(requested_context);
    let mut cache_type = if tuning.kv_cache == "q8_0" {
        "q8_0".to_string()
    } else {
        default_cache_type()
    };
    let mut fit_note = None;
    let mut placement = if vram.is_some() { "gpu" } else { "unknown" };
    let (cpu_cap, cpu_note) = if automatic {
        cpu_context_cap(model, ram_available_bytes)
    } else {
        (u32::MAX, None)
    };
    if automatic {
        if let (Some(model), Some(vram)) = (model, vram) {
            if let Some(plan) = fit_to_memory(
                effective_context,
                &cache_type,
                true,
                model,
                vram,
                ram_available_bytes,
            ) {
                if tuning.keeps_requested_context() && plan.context < effective_context {
                    // The user's size stands. Say what it costs rather than
                    // quietly charging them for it: a larger cache takes GPU
                    // memory the weights would otherwise have used.
                    let cache = cache_bytes_per_token(
                        model.kv_bytes_per_token.unwrap_or(0),
                        &plan.cache_type,
                    ) * u64::from(effective_context);
                    cache_type = plan.cache_type;
                    placement = if plan.placement == "gpu" {
                        "hybrid"
                    } else {
                        plan.placement
                    };
                    fit_note = Some(format!(
                        "Context kept at {effective_context} tokens because the context size is set to be used as written. Fitting it to GPU memory would have given {} tokens instead. Its cache needs {:.1} GB, so more of the model runs on the CPU and replies are slower; switch the setting back to fit it automatically for the faster placement.",
                        plan.context,
                        cache as f64 / 1e9
                    ));
                } else {
                    effective_context = plan.context;
                    cache_type = plan.cache_type;
                    placement = plan.placement;
                    fit_note = plan.note;
                }
            }
        }
    }
    let (speculative, spec_draft_n_max) = speculative_for(model, tuning.speculative == "off");
    let cache_reuse = if tuning.cache_reuse {
        default_cache_reuse()
    } else {
        0
    };
    let mut notes = vec![
        if cache_type == "q8_0" {
            "Weight precision and KV-cache precision are independent. The KV cache is stored as 8-bit integers (q8_0) with Flash Attention, halving cache memory versus f16 at a negligible quality cost; the runtime validates architecture support at load.".into()
        } else {
            "Weight precision and KV-cache precision are independent. Compatibility-first K/V f16 is explicit; legacy inactive cache preferences are not applied.".into()
        },
        "llama.cpp uses this model's GGUF metadata and chat template to construct its native attention, sliding-window or recurrent state.".into(),
        "Every model load uses a fresh process/cache; subsequent requests rebuild context from saved messages using the selected model's tokenizer and template.".into(),
    ];
    if speculative == DRAFT_HEAD_SPECULATIVE {
        notes.push(format!("This model carries a built-in draft head (a next-token prediction layer): it drafts {DRAFT_HEAD_TOKENS} tokens per step, and n-gram drafting adds tokens already in the context. Measured on a 27B model: prose 35 → 53 tok/s and file rewrites 35 → 88 tok/s, for about 0.9 GB more VRAM, which the memory fit reserves. The model still chooses every token; verifying several at once can flip a near-tied token, so text can differ slightly from plain decoding."));
    } else if speculative != "none" {
        notes.push(format!("Self-speculative decoding ({speculative}) drafts tokens already present in the context and verifies them in one batch. The model still chooses every token; verifying several at once can very rarely flip a near-tied token, so text can differ slightly from plain decoding. It is faster when the reply repeats context (code edits, file rewrites, tool envelopes)."));
    }
    if cache_reuse > 0 {
        notes.push(format!("Prompt-cache chunk reuse ({cache_reuse}-token minimum) keeps the unchanged tail of a transcript in the KV cache when earlier turns are pruned, so only the changed part is re-prefilled."));
    }
    let context_note = fit_note.or_else(|| {
        (effective_context < requested_context).then(|| {
            format!("Context capped at the model's advertised limit of {effective_context} tokens; the configured preference is unchanged.")
        })
    });
    if let Some(note) = &context_note {
        notes.push(note.clone());
    }
    if let Some(note) = cpu_note {
        notes.push(note);
    }
    if model.is_none() {
        notes.push("Model metadata is unavailable; no model context limit can be verified. Native load validation remains authoritative.".into());
    }
    // llama.cpp cannot create a context with a quantized value cache when
    // Flash Attention is off (measured: "failed to create context"). Settings
    // now refuse that pairing; a save from before keeps the 8-bit key cache
    // and stores values at f16 rather than failing to load.
    let cache_type_v = if !automatic && !requested.flash_attn && cache_type != "f16" {
        notes.push(format!("Flash Attention is off, which a {cache_type} value cache requires, so values are cached at f16 and keys at {cache_type}. Turn Flash Attention on to halve the value cache too."));
        "f16".to_string()
    } else {
        cache_type.clone()
    };
    if automatic {
        notes.push("The installed runtime is checked for usable devices at load time, including supported integrated GPUs. With no usable GPU, automatic CPU settings are selected; GPU initialization failures retry once on CPU. CPU operation caps context at 8192 without changing saved preferences; all physical cores are used, which measured faster than performance cores alone on a hybrid CPU.".into());
    }
    ResolvedRuntimePolicy {
        mode: if automatic { "automatic" } else { "manual" }.into(),
        architecture: model
            .map(|model| model.architecture.clone())
            .unwrap_or_else(|| "unknown".into()),
        weights_quantization: model
            .map(|model| model.quantization.clone())
            .unwrap_or_else(|| "unknown".into()),
        requested_context,
        effective_context,
        cache_type_k: cache_type,
        cache_type_v,
        flash_attention: if automatic {
            "auto"
        } else if requested.flash_attn {
            "on"
        } else {
            "off"
        }
        .into(),
        kv_offload: if automatic {
            "auto"
        } else if requested.kv_cache_gpu {
            "on"
        } else {
            "off"
        }
        .into(),
        threads: if automatic {
            0
        } else {
            requested.n_threads.max(1)
        },
        gpu_layers: if automatic {
            -1
        } else {
            requested.n_gpu_layers
        },
        batch_size: if automatic {
            0
        } else {
            requested.n_batch.max(1).min(effective_context)
        },
        cache_reuse,
        speculative,
        spec_draft_n_max,
        cpu_context_cap: cpu_cap,
        placement: placement.into(),
        cache_rebuild: "fresh_process".into(),
        notes,
        context_note,
    }
}

/// RAM for llama-server's prompt cache (`--cache-ram`, MiB). The server's
/// default of 8 GiB is kept only when free RAM allows it after the model memory
/// that lives in RAM and a 2 GiB margin for the system; otherwise half of what
/// is left, and 0 (cache off) under 256 MiB, so the cache can never push the
/// weights into paging.
pub fn prompt_cache_ram_mib(ram_available_bytes: u64, ram_side_model_bytes: u64) -> u32 {
    const MIB: u64 = 1_048_576;
    let spare = ram_available_bytes
        .saturating_sub(ram_side_model_bytes)
        .saturating_sub(2048 * MIB);
    let mib = (spare / 2 / MIB).min(8192) as u32;
    if mib < 256 {
        0
    } else {
        mib
    }
}

impl ResolvedRuntimePolicy {
    pub fn apply_to(&self, cfg: &mut InferenceConfig) {
        cfg.n_ctx = self.effective_context;
        cfg.n_batch = self.batch_size;
        cfg.n_threads = self.threads;
        if self.mode == "automatic" {
            // Manual-only launch values do not leak into automatic loads; a
            // calibrated profile sets its own afterwards.
            cfg.n_threads_batch = 0;
            cfg.poll = None;
            cfg.priority = None;
        }
        // Chosen after the policy, from the runtime's fit or a calibration, and
        // only in automatic modes: a policy never carries an earlier choice,
        // and Manual keeps llama-server's defaults.
        cfg.micro_batch = None;
        cfg.load_without_mmap = false;
        cfg.spec_ngram_length = None;
        cfg.n_gpu_layers = self.gpu_layers;
        cfg.flash_attn_auto = self.flash_attention == "auto";
        cfg.flash_attn = self.flash_attention != "off";
        cfg.kv_cache_gpu = self.kv_offload != "off";
        cfg.kv_cache_type_k = self.cache_type_k.clone();
        cfg.kv_cache_type_v = self.cache_type_v.clone();
        cfg.cache_reuse = self.cache_reuse;
        cfg.speculative = self.speculative.clone();
        cfg.spec_draft_n_max = self.spec_draft_n_max;
        cfg.runtime_policy = Some(self.clone());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub tokens_per_sec: f32,
    pub prompt_speed_tps: f32,
    pub time_to_first_token_ms: u64,
    pub prompt_tokens: u32,
    pub generated_tokens: u32,
    pub kv_cache_used: u32,
    pub kv_cache_limit: u32,
    /// Versioned visible-output timing. Absent on legacy/non-streaming metrics.
    #[serde(default)]
    pub timing: Option<OutputTiming>,
    /// Runtime-measured timings for the last completion, when reported.
    #[serde(default)]
    pub engine: Option<EngineTimings>,
    /// Why the runtime stopped: "stop" (natural end), "length" (hit the
    /// output limit), or another provider value. `None` when not reported.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTiming {
    pub basis: String,
    pub output_tps: Option<f64>,
    pub estimated: bool,
    pub token_basis: String,
    pub output_tokens: u32,
    pub first_visible_ms: Option<u64>,
    pub output_ms: u64,
    /// Observed reasoning-channel duration, never inferred from silence.
    pub thinking_ms: Option<u64>,
    pub total_ms: u64,
    /// Engine-measured decode throughput (all generated tokens, including any
    /// hidden reasoning) summed across the rounds of this turn.
    #[serde(default)]
    pub engine_output_tps: Option<f64>,
    /// Engine-measured prefill throughput for the tokens that were not cached.
    #[serde(default)]
    pub engine_prompt_tps: Option<f64>,
    /// Prompt tokens served from the KV cache across the rounds of this turn.
    #[serde(default)]
    pub cached_tokens: Option<u32>,
    /// Engine-counted generated tokens across the rounds of this turn.
    #[serde(default)]
    pub predicted_tokens: Option<u32>,
    /// Speculative decoding acceptance across the rounds of this turn.
    #[serde(default)]
    pub draft_tokens: Option<u32>,
    #[serde(default)]
    pub draft_accepted: Option<u32>,
    /// Accumulators for engine rates (never displayed directly).
    #[serde(default, skip_serializing)]
    pub engine_predicted_ms: f64,
    #[serde(default, skip_serializing)]
    pub engine_prompt_ms: f64,
    #[serde(default, skip_serializing)]
    pub engine_prompt_tokens: u32,
}

impl Default for OutputTiming {
    fn default() -> Self {
        Self {
            basis: "visible_output_v1".into(),
            output_tps: None,
            estimated: false,
            token_basis: "unavailable".into(),
            output_tokens: 0,
            first_visible_ms: None,
            output_ms: 0,
            thinking_ms: None,
            total_ms: 0,
            engine_output_tps: None,
            engine_prompt_tps: None,
            cached_tokens: None,
            predicted_tokens: None,
            draft_tokens: None,
            draft_accepted: None,
            engine_predicted_ms: 0.0,
            engine_prompt_ms: 0.0,
            engine_prompt_tokens: 0,
        }
    }
}

impl OutputTiming {
    pub fn update_rate(&mut self) {
        self.output_tps = (self.output_tokens > 0 && self.output_ms > 0).then(|| {
            (self.output_tokens as f64 * 1000.0 / self.output_ms as f64 * 10.0).round() / 10.0
        });
        if let Some(predicted) = self.predicted_tokens {
            self.engine_output_tps = (predicted > 0 && self.engine_predicted_ms > 0.0).then(|| {
                (predicted as f64 * 1000.0 / self.engine_predicted_ms * 10.0).round() / 10.0
            });
        }
        self.engine_prompt_tps =
            (self.engine_prompt_tokens > 0 && self.engine_prompt_ms > 0.0).then(|| {
                (self.engine_prompt_tokens as f64 * 1000.0 / self.engine_prompt_ms * 10.0).round()
                    / 10.0
            });
    }

    /// Fold one completion's engine timings into this turn's totals.
    pub fn add_engine(&mut self, engine: &EngineTimings) {
        self.predicted_tokens = Some(
            self.predicted_tokens
                .unwrap_or(0)
                .saturating_add(engine.predicted_tokens),
        );
        self.engine_predicted_ms += engine.predicted_ms;
        self.engine_prompt_tokens = self.engine_prompt_tokens.saturating_add(engine.prompt_tokens);
        self.engine_prompt_ms += engine.prompt_ms;
        self.cached_tokens = Some(
            self.cached_tokens
                .unwrap_or(0)
                .saturating_add(engine.cached_tokens),
        );
        if let Some(drafted) = engine.draft_tokens {
            self.draft_tokens = Some(self.draft_tokens.unwrap_or(0).saturating_add(drafted));
            self.draft_accepted = Some(
                self.draft_accepted
                    .unwrap_or(0)
                    .saturating_add(engine.draft_accepted.unwrap_or(0)),
            );
        }
        self.update_rate();
    }

    /// Combine only active visible emission spans. Time spent in another
    /// round's prompt, native reasoning, tokenizer or a tool is not output time.
    pub fn add_round(&mut self, round: &Self, round_offset_ms: u64) {
        if self.first_visible_ms.is_none() {
            self.first_visible_ms = round
                .first_visible_ms
                .map(|ms| round_offset_ms.saturating_add(ms));
        }
        if round.output_tokens > 0 {
            self.token_basis = if self.output_tokens == 0 {
                round.token_basis.clone()
            } else if self.token_basis == round.token_basis {
                self.token_basis.clone()
            } else {
                "mixed".into()
            };
            self.estimated |= round.estimated;
        }
        self.output_tokens = self.output_tokens.saturating_add(round.output_tokens);
        self.output_ms = self.output_ms.saturating_add(round.output_ms);
        if let Some(ms) = round.thinking_ms {
            self.thinking_ms = Some(self.thinking_ms.unwrap_or(0).saturating_add(ms));
        }
        if let Some(predicted) = round.predicted_tokens {
            self.predicted_tokens = Some(self.predicted_tokens.unwrap_or(0).saturating_add(predicted));
        }
        self.engine_predicted_ms += round.engine_predicted_ms;
        self.engine_prompt_tokens = self.engine_prompt_tokens.saturating_add(round.engine_prompt_tokens);
        self.engine_prompt_ms += round.engine_prompt_ms;
        if let Some(cached) = round.cached_tokens {
            self.cached_tokens = Some(self.cached_tokens.unwrap_or(0).saturating_add(cached));
        }
        if let Some(drafted) = round.draft_tokens {
            self.draft_tokens = Some(self.draft_tokens.unwrap_or(0).saturating_add(drafted));
            self.draft_accepted = Some(
                self.draft_accepted
                    .unwrap_or(0)
                    .saturating_add(round.draft_accepted.unwrap_or(0)),
            );
        }
        self.update_rate();
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            tokens_per_sec: 0.0,
            prompt_speed_tps: 0.0,
            time_to_first_token_ms: 0,
            prompt_tokens: 0,
            generated_tokens: 0,
            kv_cache_used: 0,
            kv_cache_limit: 32768,
            timing: None,
            engine: None,
            finish_reason: None,
        }
    }
}

/// §100: common inference interface. All engines implement this.
#[allow(async_fn_in_trait)]
pub trait IInferenceEngine: Send + Sync {
    fn load(&mut self, cfg: InferenceConfig) -> Result<(), InferenceError>;
    fn unload(&mut self);
    fn is_loaded(&self) -> bool;
    fn context_size(&self) -> u32;
    fn metrics(&self) -> Metrics;
    async fn generate(&self, prompt: String, max_tokens: u32) -> Result<String, InferenceError>;
    /// Token-by-token streaming; the callback receives each token (§19).
    async fn stream(
        &self,
        prompt: String,
        max_tokens: u32,
        on_token: &mut dyn FnMut(String),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Metrics, InferenceError>;
    fn tokenize(&self, text: &str) -> Vec<u32>;
}

/// Deterministic stub used until Stage 3 wires real llama.cpp.
#[derive(Debug, Default)]
pub struct StubEngine {
    loaded: bool,
    ctx: u32,
    metrics: Metrics,
}

impl StubEngine {
    pub fn new() -> Self {
        Self {
            loaded: false,
            ctx: 32768,
            metrics: Metrics::default(),
        }
    }
}

impl IInferenceEngine for StubEngine {
    fn load(&mut self, cfg: InferenceConfig) -> Result<(), InferenceError> {
        if cfg.model_path.as_os_str().is_empty() {
            return Err(InferenceError::ModelFileMissing("<empty path>".into()));
        }
        self.loaded = true;
        self.ctx = cfg.n_ctx;
        Ok(())
    }

    fn unload(&mut self) {
        self.loaded = false;
        self.metrics = Metrics::default();
    }

    fn is_loaded(&self) -> bool {
        self.loaded
    }

    fn context_size(&self) -> u32 {
        self.ctx
    }

    fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }

    async fn generate(&self, prompt: String, max_tokens: u32) -> Result<String, InferenceError> {
        if !self.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }
        let words: Vec<&str> = prompt
            .split_whitespace()
            .take(max_tokens as usize)
            .collect();
        Ok(format!(
            "[stub] echo ({} words): {}",
            words.len(),
            words.join(" ")
        ))
    }

    async fn stream(
        &self,
        prompt: String,
        max_tokens: u32,
        on_token: &mut dyn FnMut(String),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Metrics, InferenceError> {
        if !self.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }
        let mut m = Metrics {
            tokens_per_sec: 38.0,
            prompt_speed_tps: 120.0,
            time_to_first_token_ms: 45,
            ..Metrics::default()
        };
        for tok in prompt.split_whitespace().take(max_tokens as usize) {
            if is_cancelled() {
                break;
            }
            on_token(format!("{tok} "));
            m.generated_tokens += 1;
        }
        m.prompt_tokens = prompt.split_whitespace().count() as u32;
        Ok(m)
    }

    fn tokenize(&self, text: &str) -> Vec<u32> {
        // Stub: word-count based IDs; real engine returns SentencePiece/BPE IDs.
        text.split_whitespace()
            .enumerate()
            .map(|(i, _)| i as u32)
            .collect()
    }
}

/// Stage 3 placeholder. Validates config and returns actionable errors (§52)
/// until native llama.cpp bindings land.
#[derive(Debug, Default)]
pub struct LlamaCppEngine {
    loaded: bool,
    cfg: Option<InferenceConfig>,
}

impl LlamaCppEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Heuristic GGUF size check before attempting load.
    pub fn check_fits(required_gb: f64, available_gb: f64) -> Result<(), InferenceError> {
        if required_gb > available_gb {
            return Err(InferenceError::InsufficientMemory {
                required_gb,
                available_gb,
            });
        }
        Ok(())
    }
}

impl IInferenceEngine for LlamaCppEngine {
    fn load(&mut self, cfg: InferenceConfig) -> Result<(), InferenceError> {
        if !cfg.model_path.exists() {
            return Err(InferenceError::ModelFileMissing(
                cfg.model_path.display().to_string(),
            ));
        }
        // TODO(Stage 3): init llama.cpp common params, backend (CUDA/Vulkan/CPU),
        // MTMD projector if cfg.projector_path.is_some().
        self.loaded = true;
        self.cfg = Some(cfg);
        Ok(())
    }

    fn unload(&mut self) {
        // TODO(Stage 3): llama_free + backend buffer release.
        self.loaded = false;
        self.cfg = None;
    }

    fn is_loaded(&self) -> bool {
        self.loaded
    }

    fn context_size(&self) -> u32 {
        self.cfg.as_ref().map(|c| c.n_ctx).unwrap_or(0)
    }

    fn metrics(&self) -> Metrics {
        Metrics::default()
    }

    async fn generate(&self, _prompt: String, _max: u32) -> Result<String, InferenceError> {
        Err(InferenceError::Generation(
            "llama.cpp backend not wired yet (Stage 3). Use StubEngine for now.".into(),
        ))
    }

    async fn stream(
        &self,
        _prompt: String,
        _max: u32,
        _on_token: &mut dyn FnMut(String),
        _is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Metrics, InferenceError> {
        Err(InferenceError::Generation(
            "llama.cpp backend not wired yet (Stage 3). Use StubEngine for now.".into(),
        ))
    }

    fn tokenize(&self, _text: &str) -> Vec<u32> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_test_model(architecture: &str, context: u32) -> crate::models::ModelMetadata {
        serde_json::from_value(serde_json::json!({
            "id": "fixture", "name": "Fixture", "architecture": architecture,
            "quantization": "Q4_K_M", "parameters": "8B", "context_length": context,
            "vision": false, "tool_calling": false
        }))
        .unwrap()
    }

    #[test]
    fn a_model_with_a_draft_head_drafts_with_it() {
        let mut model = policy_test_model("qwen35", 32768);
        assert_eq!(speculative_for(Some(&model), false), ("ngram-simple".to_string(), None));
        model.draft_head_layers = Some(1);
        assert_eq!(speculative_for(Some(&model), false), (DRAFT_HEAD_SPECULATIVE.to_string(), Some(DRAFT_HEAD_TOKENS)));
        assert_eq!(speculative_for(Some(&model), true), ("none".to_string(), None), "the owner's off switch wins");
        model.draft_head_layers = Some(0);
        assert_eq!(speculative_for(Some(&model), false).0, "ngram-simple");
        assert_eq!(speculative_for(None, false).0, "ngram-simple");
    }

    #[test]
    fn runtime_policy_is_recomputed_for_each_model_without_mutating_preferences() {
        let requested = InferenceConfig {
            n_ctx: 32768,
            n_threads: 99,
            n_gpu_layers: 999,
            n_batch: 4096,
            ..InferenceConfig::default()
        };
        let first =
            resolve_runtime_policy(Some(&policy_test_model("gemma4", 262144)), &requested, true);
        let second =
            resolve_runtime_policy(Some(&policy_test_model("qwen2", 8192)), &requested, true);
        assert_eq!(first.effective_context, 32768);
        assert_eq!(second.effective_context, 8192);
        assert_eq!(requested.n_ctx, 32768);
        assert_eq!(first.architecture, "gemma4");
        assert_eq!(second.architecture, "qwen2");
        assert_eq!(first.weights_quantization, "Q4_K_M");
        assert_eq!(first.cache_type_k, "f16");
        assert_eq!(first.cache_type_v, "f16");
        assert_eq!(second.cache_type_k, "f16");
        assert_eq!(second.flash_attention, "auto");
        assert_eq!(second.gpu_layers, -1);
        assert_eq!(
            second.threads, 0,
            "native runtime chooses automatic thread count"
        );
        assert_eq!(
            second.batch_size, 0,
            "automatic mode leaves prompt batching to the runtime default"
        );
        assert_eq!(second.cache_reuse, 256);
        assert_eq!(second.speculative, "ngram-simple");
        assert_eq!(second.cache_rebuild, "fresh_process");
    }

    #[test]
    fn runtime_tuning_preferences_select_cache_precision_and_speculation() {
        let requested = InferenceConfig::default();
        let tuning = crate::settings::RuntimeSettings {
            speculative: "off".into(),
            kv_cache: "q8_0".into(),
            cache_reuse: false,
            ..Default::default()
        };
        let policy = resolve_runtime_policy_with(None, &requested, true, &tuning);
        assert_eq!(policy.cache_type_k, "q8_0");
        assert_eq!(policy.cache_type_v, "q8_0");
        assert_eq!(policy.speculative, "none");
        assert_eq!(policy.cache_reuse, 0);
        let mut applied = requested.clone();
        policy.apply_to(&mut applied);
        assert_eq!(applied.kv_cache_type_k, "q8_0");
        assert_eq!(applied.speculative, "none");
        assert_eq!(applied.cache_reuse, 0);
        let unknown = crate::settings::RuntimeSettings {
            kv_cache: "q4_0".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_runtime_policy_with(None, &requested, true, &unknown).cache_type_k,
            "f16",
            "only the validated q8_0 option changes cache precision"
        );
    }

    #[test]
    fn context_and_cache_are_fitted_to_the_gpu_before_loading() {
        let gib = 1024u64 * 1024 * 1024;
        let card = VramState {
            total_bytes: 12227 * 1024 * 1024,
            used_bytes: 1360 * 1024 * 1024,
        };
        let tuning = crate::settings::RuntimeSettings::default();
        let requested = InferenceConfig {
            n_ctx: 32768,
            ..InferenceConfig::default()
        };
        // 8B Q4_K_M (5.0 GB, 147 KB/token): 32K f16 fits, nothing changes.
        let mut small = policy_test_model("qwen3", 40960);
        small.weights_bytes = Some(5_027_783_488);
        small.kv_bytes_per_token = Some(147_456);
        let policy = resolve_runtime_policy_fitted(Some(&small), &requested, true, &tuning, Some(card));
        assert_eq!(policy.effective_context, 32768);
        assert_eq!(policy.cache_type_k, "f16");
        // 14B Q4_K_M (9.0 GB, 196 KB/token): 32K needs 6.4 GB of f16 cache and
        // spills; the fit keeps the model resident with a smaller context.
        let mut large = policy_test_model("qwen2", 32768);
        large.weights_bytes = Some(8_988_110_272);
        large.kv_bytes_per_token = Some(196_608);
        let policy = resolve_runtime_policy_fitted(Some(&large), &requested, true, &tuning, Some(card));
        assert!(policy.effective_context < 32768 && policy.effective_context >= 4096, "{}", policy.effective_context);
        let bytes = large.weights_bytes.unwrap()
            + cache_bytes_per_token(196_608, &policy.cache_type_k) * policy.effective_context as u64;
        assert!(bytes < card.total_bytes - card.used_bytes, "fitted plan must be resident");
        assert!(policy.notes.iter().any(|note| note.contains("GPU memory")));
        // The same model with the f16-only preference still fits, at a
        // smaller f16 context rather than an 8-bit cache.
        let f16_only = resolve_runtime_policy_fitted(
            Some(&large), &requested, true,
            &crate::settings::RuntimeSettings { kv_cache: "f16".into(), ..tuning.clone() }, Some(card),
        );
        assert_eq!(f16_only.cache_type_k, if policy.cache_type_k == "q8_0" { "q8_0" } else { "f16" });
        // Another application holding a few hundred MB more: the context
        // shrinks by what is missing, in 1,024-token steps, instead of
        // halving from 8K to 4K (which left a coding agent a 4K window).
        let busier = VramState { used_bytes: 1600 * 1024 * 1024, ..card };
        let tight = resolve_runtime_policy_fitted(Some(&large), &requested, true, &tuning, Some(busier));
        assert_eq!(tight.placement, "gpu");
        assert_eq!(tight.effective_context, 10240, "halving would have given 8192, the old margins 6144");
        assert_eq!(tight.cache_type_k, "q8_0");
        // The measured boundary on that card with 1,721 MiB held by other
        // applications: 11,264 tokens ran fully on the GPU, 12,288 spilled.
        // The plan must stay inside it and still leave a usable window.
        let measured = VramState { used_bytes: 1721 * 1024 * 1024, ..card };
        let plan = resolve_runtime_policy_fitted(Some(&large), &requested, true, &tuning, Some(measured));
        assert!(plan.effective_context >= 8192 && plan.effective_context <= 11264, "{}", plan.effective_context);
        assert_eq!(plan.cache_type_k, "q8_0");
        let bytes = large.weights_bytes.unwrap()
            + cache_bytes_per_token(196_608, &tight.cache_type_k) * tight.effective_context as u64;
        assert!(bytes < busier.total_bytes - busier.used_bytes, "fitted plan must be resident");
        // Other applications holding ~2.4 GB: the weights still fit, only the
        // smallest cache misses the budget by less than the safety margin.
        // The model stays on the GPU at the smallest window instead of a
        // hybrid plan with a large cache that spills layers to the CPU.
        let near = VramState { used_bytes: 2450 * 1024 * 1024, ..card };
        let near_fit = resolve_runtime_policy_fitted(Some(&large), &requested, true, &tuning, Some(near));
        assert_eq!(near_fit.placement, "gpu");
        assert_eq!(near_fit.effective_context, 4096);
        assert_eq!(near_fit.cache_type_k, "q8_0");
        // A saved window below 4K is honoured, not raised.
        let small_request = InferenceConfig { n_ctx: 2048, ..InferenceConfig::default() };
        let starved_gpu = VramState { used_bytes: 11000 * 1024 * 1024, ..card };
        assert!(resolve_runtime_policy_fitted(Some(&large), &small_request, true, &tuning, Some(starved_gpu)).effective_context <= 2048);
        // Manual mode and unknown memory never change the request.
        assert_eq!(resolve_runtime_policy_fitted(Some(&large), &requested, false, &tuning, Some(card)).effective_context, 32768);
        assert_eq!(resolve_runtime_policy_fitted(Some(&large), &requested, true, &tuning, None).effective_context, 32768);
        assert_eq!(policy.placement, "gpu");
        // A 32B Q4 (19 GB) on the same card: hybrid, with the cache kept small
        // so the GPU holds as many layers as possible, and RAM checked.
        let mut big = policy_test_model("qwen2", 32768);
        big.weights_bytes = Some(19 * gib);
        big.kv_bytes_per_token = Some(262_144);
        let policy = resolve_runtime_policy_for_machine(Some(&big), &requested, true, &tuning, Some(card), Some(40 * gib));
        assert_eq!(policy.placement, "hybrid");
        assert_eq!(policy.cache_type_k, "q8_0");
        assert!(policy.effective_context >= 4096 && policy.effective_context <= 32768);
        assert!(policy.notes.iter().any(|note| note.contains("run on the CPU")));
        // The same model on a 16 GB machine cannot hold the spilled layers.
        let starved = resolve_runtime_policy_for_machine(Some(&big), &requested, true, &tuning, Some(card), Some(6 * gib));
        assert_eq!(starved.placement, "oversubscribed");
        assert!(starved.notes.iter().any(|note| note.contains("cannot hold")));
    }

    #[test]
    fn cpu_context_cap_follows_free_ram() {
        let gib = 1024u64 * 1024 * 1024;
        let mut eight_b = policy_test_model("qwen3", 40960);
        eight_b.weights_bytes = Some(5 * gib);
        eight_b.kv_bytes_per_token = Some(147_456);
        // 32 GB laptop with ~24 GB free: the comfort cap applies.
        assert_eq!(cpu_context_cap(Some(&eight_b), Some(24 * gib)), (8192, None));
        // 7 GiB free: 5 GiB weights + overheads leave ~0.7 GB, so 4096 tokens.
        let (cap, note) = cpu_context_cap(Some(&eight_b), Some(7 * gib));
        assert_eq!(cap, 4096);
        assert!(note.unwrap().contains("free system RAM"));
        // A 14B model on a machine with 6 GB free cannot be resident.
        let mut fourteen_b = policy_test_model("qwen2", 32768);
        fourteen_b.weights_bytes = Some(9 * gib);
        fourteen_b.kv_bytes_per_token = Some(196_608);
        let (cap, note) = cpu_context_cap(Some(&fourteen_b), Some(6 * gib));
        assert_eq!(cap, 2048);
        assert!(note.unwrap().contains("tight"));
        // Unknown inputs keep the default cap.
        assert_eq!(cpu_context_cap(None, Some(64 * gib)), (8192, None));
        assert_eq!(cpu_context_cap(Some(&eight_b), None), (8192, None));
        // The resolved policy carries the cap for the CPU fallback and never
        // changes the GPU plan because of RAM.
        let requested = InferenceConfig { n_ctx: 32768, ..InferenceConfig::default() };
        let tuning = crate::settings::RuntimeSettings::default();
        let policy = resolve_runtime_policy_for_machine(Some(&eight_b), &requested, true, &tuning, None, Some(7 * gib));
        assert_eq!(policy.cpu_context_cap, 4096);
        assert_eq!(policy.effective_context, 32768);
        let mut applied = requested.clone();
        policy.apply_to(&mut applied);
        let cpu = crate::runtime_selection::cpu_configuration(applied, "test");
        assert_eq!(cpu.n_ctx, 4096);
    }

    #[test]
    fn engine_timings_parse_llama_server_timings_and_rates() {
        let timings = serde_json::json!({
            "prompt_n": 8133, "prompt_ms": 2496.293, "prompt_per_second": 3258.0,
            "predicted_n": 80, "predicted_ms": 1369.927, "cache_n": 552,
            "draft_n": 64, "draft_n_accepted": 3
        });
        let engine = EngineTimings::from_json(&timings).unwrap();
        assert_eq!(engine.prompt_tokens, 8133);
        assert_eq!(engine.predicted_tokens, 80);
        assert_eq!(engine.cached_tokens, 552);
        assert_eq!(engine.prompt_tps, Some(3258.0));
        assert_eq!(engine.predicted_tps, Some(58.4));
        assert_eq!(engine.draft_tokens, Some(64));
        assert_eq!(engine.draft_accepted, Some(3));
        assert!(EngineTimings::from_json(&serde_json::json!({})).is_none());
        assert!(EngineTimings::from_json(&serde_json::Value::Null).is_none());
        let mut timing = OutputTiming::default();
        timing.add_engine(&engine);
        timing.add_engine(&EngineTimings {
            predicted_tokens: 20,
            predicted_ms: 200.0,
            ..EngineTimings::default()
        });
        assert_eq!(timing.predicted_tokens, Some(100));
        assert_eq!(timing.cached_tokens, Some(552));
        assert_eq!(timing.engine_output_tps, Some(63.7));
        assert_eq!(timing.engine_prompt_tps, Some(3258.0));
        let stored: OutputTiming =
            serde_json::from_str(&serde_json::to_string(&timing).unwrap()).unwrap();
        assert_eq!(stored.engine_output_tps, Some(63.7));
        let legacy: OutputTiming = serde_json::from_str(
            r#"{"basis":"visible_output_v1","output_tps":20.0,"estimated":false,"token_basis":"tokenizer","output_tokens":20,"first_visible_ms":100,"output_ms":1000,"thinking_ms":null,"total_ms":1200}"#,
        )
        .unwrap();
        assert!(legacy.engine_output_tps.is_none());
    }

    #[test]
    fn manual_runtime_honors_active_controls_but_not_legacy_inactive_cache_type() {
        let mut requested = InferenceConfig {
            n_ctx: 4096,
            n_threads: 6,
            n_gpu_layers: 0,
            n_batch: 256,
            flash_attn: false,
            kv_cache_gpu: false,
            kv_cache_type_k: "q8_0".into(),
            kv_cache_type_v: "q4_0".into(),
            // A choice left over from an automatic load.
            micro_batch: Some(1024),
            load_without_mmap: true,
            ..InferenceConfig::default()
        };
        let policy = resolve_runtime_policy(None, &requested, false);
        policy.apply_to(&mut requested);
        assert_eq!(requested.micro_batch, None, "Manual keeps llama-server's micro-batch");
        assert!(!requested.load_without_mmap, "Manual keeps llama-server's load mode");
        assert_eq!(requested.n_threads, 6);
        assert_eq!(requested.n_gpu_layers, 0);
        assert_eq!(requested.n_batch, 256);
        assert!(!requested.flash_attn_auto);
        assert!(!requested.flash_attn);
        assert!(!requested.kv_cache_gpu);
        assert_eq!(requested.kv_cache_type_k, "f16");
        assert_eq!(requested.kv_cache_type_v, "f16");
        assert_eq!(requested.runtime_policy.unwrap().mode, "manual");
    }

    #[test]
    fn template_capabilities_are_read_from_the_runtime_props() {
        let props = serde_json::json!({"chat_template_caps": {
            "supports_tools": false, "supports_tool_calls": false, "supports_system_role": true
        }});
        let caps = TemplateCaps::from_props(&props).unwrap();
        assert_eq!(caps.tools_supported(), Support::No);
        assert_eq!(caps.system_role, Support::Yes);
        assert_eq!(caps.parallel_tool_calls, Support::Unknown, "absent means unknown, never no");
        let with_calls = serde_json::json!({"chat_template_caps": {"supports_tools": false, "supports_tool_calls": true}});
        assert_eq!(TemplateCaps::from_props(&with_calls).unwrap().tools_supported(), Support::Yes);
        assert_eq!(TemplateCaps::from_props(&serde_json::json!({"chat_template_caps": {}})).unwrap().tools_supported(), Support::Unknown);
        assert!(TemplateCaps::from_props(&serde_json::json!({})).is_none());
    }

    #[test]
    fn the_prompt_cache_never_takes_ram_the_model_needs() {
        const GIB: u64 = 1_073_741_824;
        assert_eq!(prompt_cache_ram_mib(40 * GIB, 0), 8192, "plenty of RAM keeps the server default");
        assert_eq!(prompt_cache_ram_mib(12 * GIB, 6 * GIB), 2048, "half of what is left after the model and 2 GiB");
        assert_eq!(prompt_cache_ram_mib(4 * GIB, 2 * GIB), 0, "no room: the cache is off");
        assert_eq!(prompt_cache_ram_mib(0, 5 * GIB), 0);
    }

    #[test]
    fn an_old_manual_save_never_pairs_a_quantized_value_cache_with_flash_attention_off() {
        let mut requested = InferenceConfig {
            n_ctx: 4096,
            n_threads: 6,
            flash_attn: false,
            ..InferenceConfig::default()
        };
        let tuning = crate::settings::RuntimeSettings {
            kv_cache: "q8_0".into(),
            ..Default::default()
        };
        let policy = resolve_runtime_policy_with(None, &requested, false, &tuning);
        assert_eq!(policy.cache_type_k, "q8_0");
        assert_eq!(policy.cache_type_v, "f16", "llama.cpp cannot create this context");
        assert!(policy.notes.iter().any(|note| note.contains("Flash Attention is off")));
        policy.apply_to(&mut requested);
        assert_eq!(requested.kv_cache_type_v, "f16");

        requested.flash_attn = true;
        let policy = resolve_runtime_policy_with(None, &requested, false, &tuning);
        assert_eq!(policy.cache_type_v, "q8_0");
    }

    #[test]
    fn output_timing_aggregates_active_spans_not_tool_or_prompt_waits() {
        let mut total = OutputTiming::default();
        let first = OutputTiming {
            output_tokens: 10,
            output_ms: 500,
            first_visible_ms: Some(1000),
            token_basis: "tokenizer".into(),
            thinking_ms: Some(700),
            total_ms: 1700,
            ..OutputTiming::default()
        };
        let second = OutputTiming {
            output_tokens: 20,
            output_ms: 1000,
            first_visible_ms: Some(3000),
            token_basis: "character_estimate".into(),
            estimated: true,
            total_ms: 4200,
            ..OutputTiming::default()
        };
        total.add_round(&first, 200);
        total.add_round(&second, 6000);
        assert_eq!(total.first_visible_ms, Some(1200));
        assert_eq!(total.output_ms, 1500);
        assert_eq!(total.output_tokens, 30);
        assert_eq!(total.output_tps, Some(20.0));
        assert_eq!(total.thinking_ms, Some(700));
        assert_eq!(total.token_basis, "mixed");
        assert!(total.estimated);
    }

    #[tokio::test]
    async fn stub_requires_load_before_generate() {
        let e = StubEngine::new();
        let err = e.generate("hi".into(), 10).await.unwrap_err();
        assert!(matches!(err, InferenceError::ModelNotLoaded));
    }

    #[tokio::test]
    async fn stub_streams_and_counts_tokens() {
        let mut e = StubEngine::new();
        e.load(InferenceConfig::default()).unwrap();
        let mut toks = vec![];
        let m = e
            .stream("hello world foo".into(), 10, &mut |t| toks.push(t), &|| {
                false
            })
            .await
            .unwrap();
        assert_eq!(m.generated_tokens, 3);
        assert_eq!(toks.len(), 3);
    }

    #[test]
    fn oom_error_message_is_actionable() {
        let e = InferenceError::InsufficientMemory {
            required_gb: 15.0,
            available_gb: 11.2,
        };
        let s = e.to_string();
        assert!(
            s.contains("Hybrid"),
            "message should suggest hybrid mode: {s}"
        );
    }

    #[test]
    fn llamacpp_missing_file_gives_path() {
        let mut e = LlamaCppEngine::new();
        let mut cfg = InferenceConfig::default();
        cfg.model_path = PathBuf::from("does-not-exist.gguf");
        let err = e.load(cfg).unwrap_err();
        assert!(matches!(err, InferenceError::ModelFileMissing(_)));
    }
}
