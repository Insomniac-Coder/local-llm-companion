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
            }
        }
    }

    impl std::error::Error for InferenceError {}
}

/// Sampling / loading parameters (§49 Inference + §13 advanced panel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub model_path: PathBuf,
    pub projector_path: Option<PathBuf>,
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_threads: u32,
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
            n_threads: 8,
            n_gpu_layers: -1,
            flash_attn: true,
            flash_attn_auto: true,
            kv_cache_gpu: true,
            kv_cache_type_k: default_cache_type(),
            kv_cache_type_v: default_cache_type(),
            runtime_policy: None,
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
    pub batch_size: u32,
    pub cache_rebuild: String,
    pub notes: Vec<String>,
}

pub fn resolve_runtime_policy(
    model: Option<&crate::models::ModelMetadata>,
    requested: &InferenceConfig,
    automatic: bool,
) -> ResolvedRuntimePolicy {
    let requested_context = requested.n_ctx.max(1);
    let effective_context = model
        .map(|model| requested_context.min(model.context_length.max(1)))
        .unwrap_or(requested_context);
    let mut notes = vec![
        "Weight precision and KV-cache precision are independent. Compatibility-first K/V f16 is explicit; legacy inactive cache preferences are not applied.".into(),
        "llama.cpp uses this model's GGUF metadata and chat template to construct its native attention, sliding-window or recurrent state.".into(),
        "Every model load uses a fresh process/cache; subsequent requests rebuild context from saved messages using the selected model's tokenizer and template.".into(),
    ];
    if effective_context < requested_context {
        notes.push(format!("Context capped at the model's advertised limit of {effective_context} tokens; the configured preference is unchanged."));
    }
    if model.is_none() {
        notes.push("Model metadata is unavailable; no model context limit can be verified. Native load validation remains authoritative.".into());
    }
    if automatic {
        notes.push("CPU thread count, GPU placement and Flash Attention compatibility are selected by the native runtime; the configured context cap is retained.".into());
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
        cache_type_k: default_cache_type(),
        cache_type_v: default_cache_type(),
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
            512.min(effective_context)
        } else {
            requested.n_batch.max(1).min(effective_context)
        },
        cache_rebuild: "fresh_process".into(),
        notes,
    }
}

impl ResolvedRuntimePolicy {
    pub fn apply_to(&self, cfg: &mut InferenceConfig) {
        cfg.n_ctx = self.effective_context;
        cfg.n_batch = self.batch_size;
        cfg.n_threads = self.threads;
        cfg.n_gpu_layers = self.gpu_layers;
        cfg.flash_attn_auto = self.flash_attention == "auto";
        cfg.flash_attn = self.flash_attention != "off";
        cfg.kv_cache_gpu = self.kv_offload != "off";
        cfg.kv_cache_type_k = self.cache_type_k.clone();
        cfg.kv_cache_type_v = self.cache_type_v.clone();
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
        }
    }
}

impl OutputTiming {
    pub fn update_rate(&mut self) {
        self.output_tps = (self.output_tokens > 0 && self.output_ms > 0).then(|| {
            (self.output_tokens as f64 * 1000.0 / self.output_ms as f64 * 10.0).round() / 10.0
        });
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
        assert_eq!(second.batch_size, 512);
        assert_eq!(second.cache_rebuild, "fresh_process");
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
            ..InferenceConfig::default()
        };
        let policy = resolve_runtime_policy(None, &requested, false);
        policy.apply_to(&mut requested);
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
