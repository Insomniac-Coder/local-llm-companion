//! Automatic execution placement uses the selected runtime's device list, not
//! NVIDIA-only telemetry. Absence of a telemetry provider is not absence of a GPU.
use crate::inference::{thiserror_stub::InferenceError, InferenceConfig};
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Devices {
    Available,
    None,
    Unknown,
}

pub fn parse_devices(output: &str, successful: bool) -> Devices {
    if !successful {
        return Devices::Unknown;
    }
    let Some((_, entries)) = output.split_once("Available devices:") else {
        return Devices::Unknown;
    };
    let mut unknown = false;
    for line in entries
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        // Native device IDs are backend-independent: CUDA0, Vulkan0, SYCL0,
        // Metal, etc. Do not require dedicated VRAM or an NVIDIA vendor name.
        if line.split_once(':').is_some_and(|(id, desc)| {
            !id.is_empty() && !id.contains(char::is_whitespace) && !desc.trim().is_empty()
        }) {
            return Devices::Available;
        }
        unknown = true;
    }
    if unknown {
        Devices::Unknown
    } else {
        Devices::None
    }
}

pub fn cpu_configuration(mut cfg: InferenceConfig, reason: &str) -> InferenceConfig {
    cfg.n_gpu_layers = 0;
    cfg.kv_cache_gpu = false;
    cfg.flash_attn_auto = false;
    cfg.flash_attn = false;
    cfg.n_threads = 0; // Let the CPU runtime choose; never force a machine-specific value.
    // The policy sized this cap from free system RAM (8192 when plentiful).
    let cap = cfg
        .runtime_policy
        .as_ref()
        .map(|policy| policy.cpu_context_cap)
        .unwrap_or(8192)
        .max(1024);
    cfg.n_ctx = cfg.n_ctx.min(cap);
    // A CPU cache pays a prompt-processing penalty for q8_0 (measured ~15%);
    // f16 is the better CPU default regardless of the GPU-oriented preference.
    cfg.kv_cache_type_k = "f16".into();
    cfg.kv_cache_type_v = "f16".into();
    // The micro-batch and the load mode were chosen for the GPU placement. On
    // CPU the runtime's micro-batch (512) measured best, and CPU-only loads
    // keep the default load mode.
    cfg.micro_batch = None;
    cfg.load_without_mmap = false;
    // The draft head was measured on the GPU only; the CPU keeps n-gram
    // drafting, which costs nothing when it finds no match.
    if cfg.speculative == crate::inference::DRAFT_HEAD_SPECULATIVE {
        cfg.speculative = crate::inference::default_speculative();
    }
    cfg.spec_draft_n_max = None;
    // The n-gram length was chosen for a GPU placement; CPU-only keeps the
    // runtime's default until measured.
    cfg.spec_ngram_length = None;
    // On CPU the whole model lives in RAM: the prompt cache gets only what is
    // left after it.
    let weights = std::fs::metadata(&cfg.model_path).map(|m| m.len()).unwrap_or(0);
    let ram_available = (crate::hardware::detect().ram.available_gb * 1_073_741_824.0) as u64;
    cfg.cache_ram_mib = Some(crate::inference::prompt_cache_ram_mib(
        ram_available,
        weights.saturating_add(1_073_741_824),
    ));
    // Measured on the bundled CPU backend (docs/PERFORMANCE.md): the runtime's
    // default batch prefilled slightly faster than the old 128 cap, so an
    // automatic (0) preference stays automatic. An explicit manual value is
    // still bounded by the context.
    if cfg.n_batch > 0 {
        cfg.n_batch = cfg.n_batch.min(cfg.n_ctx).max(1);
    }
    if let Some(policy) = &mut cfg.runtime_policy {
        policy.gpu_layers = 0;
        policy.kv_offload = "off".into();
        policy.flash_attention = "off".into();
        policy.threads = 0;
        policy.effective_context = cfg.n_ctx;
        policy.batch_size = cfg.n_batch;
        policy.cache_type_k = "f16".into();
        policy.cache_type_v = "f16".into();
        policy.placement = "cpu".into();
        policy.speculative = cfg.speculative.clone();
        policy.spec_draft_n_max = None;
        policy.notes.retain(|note| {
            !note.starts_with("This model carries a built-in draft head") && !note.starts_with(crate::runtime_fit::NGRAM_LENGTH_NOTE_PREFIX)
        });
        policy.notes.retain(|note| {
            !note.starts_with(crate::runtime_fit::MICRO_BATCH_NOTE_PREFIX)
                && !note.starts_with(crate::runtime_fit::LOAD_MODE_NOTE_PREFIX)
        });
        policy.notes.push(format!("CPU mode selected automatically: {reason}. GPU, cache and projector offloading are disabled. Context is capped at 8192 for CPU operation; saved preferences are unchanged. Responses are slower on CPU: prefer a 4B-8B model at Q4_K_M or a small-active-parameter MoE model, keep Reasoning off unless needed, and rely on the prompt cache (only new text is processed each turn)."));
    }
    cfg
}

/// The configuration without the micro-batch and load mode chosen for the GPU
/// placement (`runtime_fit`): the runtime's defaults for both, and none of
/// their notes.
pub fn without_load_tuning(mut cfg: InferenceConfig) -> InferenceConfig {
    cfg.micro_batch = None;
    cfg.load_without_mmap = false;
    if let Some(policy) = cfg.runtime_policy.as_mut() {
        policy.notes.retain(|note| {
            !note.starts_with(crate::runtime_fit::MICRO_BATCH_NOTE_PREFIX)
                && !note.starts_with(crate::runtime_fit::LOAD_MODE_NOTE_PREFIX)
        });
    }
    cfg
}

pub fn gpu_startup_failure(error: &InferenceError) -> bool {
    let InferenceError::Generation(text) = error else {
        return false;
    };
    let text = text.to_lowercase();
    // Port collisions and invalid model files must not become CPU retries merely
    // because the worker's earlier log mentions a GPU.
    if [
        "already in use",
        "cannot start a fresh model runtime",
        "invalid magic",
        "unsupported model architecture",
        "failed to open model",
        "unknown argument",
        "unrecognized argument",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        return false;
    }
    text.lines().any(|line| {
        [
            "cuda error",
            "cudagetdevicecount failed",
            "cuda driver",
            "no cuda-capable device",
            "failed to initialize cuda",
            "cudamalloc failed",
            "failed to allocate cuda",
            // A pinned copy of the weights that cannot be allocated (loading
            // without mmap): "unable to allocate CUDA_Host buffer".
            "unable to allocate cuda",
            "unable to allocate vulkan",
            "no gpu devices",
            "no usable gpu",
            "vk_error",
            "hiperror",
            "sycl exception",
        ]
        .iter()
        .any(|s| line.contains(s))
            || ([
                "ggml_cuda",
                "ggml_vulkan",
                "ggml_hip",
                "ggml_sycl",
                "ggml_metal",
            ]
            .iter()
            .any(|s| line.contains(s))
                && [
                    "failed",
                    "error",
                    "out of memory",
                    "not supported",
                    "no device",
                ]
                .iter()
                .any(|s| line.contains(s)))
    })
}

/// Both branches launch the same installed application/runtime. A failed GPU
/// worker must be fully stopped by `launch` before the single CPU retry begins.
pub async fn load_with_fallback<T, F, Fut>(
    cfg: InferenceConfig,
    automatic: bool,
    devices: Devices,
    mut launch: F,
) -> Result<(T, Option<String>), InferenceError>
where
    F: FnMut(InferenceConfig) -> Fut,
    Fut: Future<Output = Result<T, InferenceError>>,
{
    if !automatic {
        return launch(cfg).await.map(|result| (result, None));
    }
    if devices == Devices::None {
        let reason = "the installed runtime reports no usable GPU";
        return launch(cpu_configuration(cfg, reason))
            .await
            .map(|result| (result, Some(format!("Running on CPU: {reason}."))));
    }
    let tuned = cfg.load_without_mmap || cfg.micro_batch.is_some_and(|size| size > crate::runtime_fit::DEFAULT_MICRO_BATCH);
    let first = match launch(cfg.clone()).await {
        Ok(result) => return Ok((result, None)),
        Err(error) if gpu_startup_failure(&error) => error,
        Err(error) => return Err(error),
    };
    // A larger micro-batch (compute buffer) or loading without mmap (a pinned
    // copy of the weights in RAM) asks for more memory than a plain start:
    // the GPU gets one more try without them before it is given up.
    let first = if tuned {
        tracing::warn!("GPU runtime startup failed with the chosen micro-batch or load mode; retrying once on the GPU without them: {first}");
        match launch(without_load_tuning(cfg.clone())).await {
            Ok(result) => {
                return Ok((
                    result,
                    Some("The GPU could not start with the chosen micro-batch or load mode, so the model runs on the GPU with the runtime's defaults for both. Prompts may be read more slowly.".into()),
                ))
            }
            Err(error) if gpu_startup_failure(&error) => error,
            Err(error) => return Err(error),
        }
    } else {
        first
    };
    tracing::warn!("GPU runtime startup failed; retrying this model once on CPU: {first}");
    match launch(cpu_configuration(cfg, "GPU initialization or allocation failed")).await {
        Ok(result) => Ok((result, Some("GPU startup failed; the model is now running on CPU. Responses may be slower.".into()))),
        Err(cpu) => Err(InferenceError::Generation(format!("Automatic GPU startup and CPU fallback both failed. CPU attempt: {cpu}. Original GPU attempt: {first}. Saved conversations were not changed."))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> InferenceConfig {
        let mut cfg = InferenceConfig::default();
        crate::inference::resolve_runtime_policy(None, &cfg, true).apply_to(&mut cfg);
        cfg
    }
    #[test]
    fn device_discovery_accepts_integrated_gpus_and_unknown_is_not_absent() {
        for name in [
            "CUDA0: NVIDIA RTX (12000 MiB)",
            "Vulkan0: Intel Iris Xe (shared memory)",
            "Vulkan0: AMD Radeon Graphics",
            "SYCL0: Intel integrated GPU",
            "Metal: Apple GPU",
        ] {
            assert_eq!(
                parse_devices(&format!("Available devices:\n  {name}\n"), true),
                Devices::Available
            );
        }
        assert_eq!(parse_devices("Available devices:\n", true), Devices::None);
        for (text, success) in [
            ("", true),
            ("Available devices:\n", false),
            ("Available devices:\nunknown output", true),
        ] {
            assert_eq!(parse_devices(text, success), Devices::Unknown);
        }
    }
    #[tokio::test]
    async fn no_gpu_uses_cpu_automatically_without_changing_saved_preferences() {
        let cfg = configured();
        let (active, notice) =
            load_with_fallback(cfg.clone(), true, Devices::None, |c| async { Ok(c) })
                .await
                .unwrap();
        assert_eq!(active.n_gpu_layers, 0);
        assert!(!active.kv_cache_gpu);
        assert!(!active.flash_attn);
        assert_eq!(active.n_ctx, 8192);
        assert_eq!(
            active.n_batch, 0,
            "automatic batch stays the runtime default on CPU (measured faster than a 128 cap)"
        );
        let mut manual = cfg.clone();
        manual.n_batch = 4096;
        assert_eq!(
            cpu_configuration(manual, "test").n_batch,
            8192.min(4096),
            "an explicit batch is bounded by the CPU context, not silently replaced"
        );
        assert_eq!(cfg.n_ctx, 32768);
        assert_eq!(cfg.n_gpu_layers, -1);
        assert!(notice.unwrap().contains("CPU"));
        assert_eq!(active.runtime_policy.unwrap().effective_context, 8192);
    }
    #[test]
    fn the_cpu_fallback_drops_the_gpu_micro_batch_and_load_mode() {
        let mut cfg = configured();
        cfg.micro_batch = Some(1024);
        cfg.load_without_mmap = true;
        if let Some(policy) = cfg.runtime_policy.as_mut() {
            policy.notes.push(format!("{}1024 tokens: test.", crate::runtime_fit::MICRO_BATCH_NOTE_PREFIX));
            policy.notes.push(format!("{}: test.", crate::runtime_fit::LOAD_MODE_NOTE_PREFIX));
        }
        let cpu = cpu_configuration(cfg, "test");
        assert_eq!(cpu.micro_batch, None);
        assert!(!cpu.load_without_mmap);
        let notes = cpu.runtime_policy.unwrap().notes;
        assert!(!notes.iter().any(|note| note.starts_with("Micro-batch") || note.starts_with("Loading without mmap")), "{notes:?}");
        assert!(notes.iter().any(|note| note.starts_with("CPU mode selected automatically")));
    }

    #[test]
    fn the_cpu_fallback_keeps_ngram_drafting_but_not_the_draft_head() {
        let mut cfg = configured();
        cfg.speculative = crate::inference::DRAFT_HEAD_SPECULATIVE.into();
        cfg.spec_draft_n_max = Some(3);
        let cpu = cpu_configuration(cfg, "test");
        assert_eq!(cpu.speculative, "ngram-simple");
        assert_eq!(cpu.spec_draft_n_max, None);
        let mut split = configured();
        split.spec_ngram_length = Some(24);
        assert_eq!(cpu_configuration(split, "test").spec_ngram_length, None, "CPU-only keeps the default length");
        let mut off = configured();
        off.speculative = "none".into();
        assert_eq!(cpu_configuration(off, "test").speculative, "none", "drafting the owner turned off stays off");
    }

    #[tokio::test]
    async fn working_gpu_and_manual_overrides_are_preserved() {
        for (automatic, devices) in [
            (true, Devices::Available),
            (true, Devices::Unknown),
            (false, Devices::None),
        ] {
            let (active, notice) =
                load_with_fallback(configured(), automatic, devices, |c| async { Ok(c) })
                    .await
                    .unwrap();
            assert_eq!(active.n_gpu_layers, -1);
            assert!(notice.is_none());
        }
    }
    #[tokio::test]
    async fn gpu_failure_retries_once_and_returns_effective_cpu_configuration() {
        let mut attempts = vec![];
        let (active, notice) = load_with_fallback(configured(), true, Devices::Available, |cfg| {
            attempts.push(cfg.n_gpu_layers);
            async move {
                if cfg.n_gpu_layers == 0 {
                    Ok(cfg)
                } else {
                    Err(InferenceError::Generation(
                        "CUDA error: out of memory".into(),
                    ))
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(attempts, [-1, 0]);
        assert_eq!(active.n_gpu_layers, 0);
        assert!(notice.unwrap().contains("GPU startup failed"));
    }
    #[tokio::test]
    async fn failures_are_bounded_and_unrelated_errors_are_not_retried() {
        for error in [
            "port 3888 is already in use",
            "unsupported model architecture",
            "invalid magic",
            "unknown argument --device",
            "CPU memory allocation failed",
        ] {
            let mut attempts = 0;
            let result: Result<((), Option<String>), _> =
                load_with_fallback(configured(), true, Devices::Unknown, |_| {
                    attempts += 1;
                    async { Err(InferenceError::Generation(error.into())) }
                })
                .await;
            assert!(result.is_err());
            assert_eq!(attempts, 1);
        }
        let mut attempts = 0;
        let result: Result<((), Option<String>), _> =
            load_with_fallback(configured(), true, Devices::Available, |_| {
                attempts += 1;
                async {
                    Err(InferenceError::Generation(
                        "CUDA error: device unavailable".into(),
                    ))
                }
            })
            .await;
        assert_eq!(attempts, 2);
        assert!(result.unwrap_err().to_string().contains("both failed"));
    }

    #[tokio::test]
    async fn a_gpu_start_that_fails_with_the_load_tuning_retries_on_the_gpu_without_it() {
        let mut cfg = configured();
        cfg.micro_batch = Some(1024);
        cfg.load_without_mmap = true;
        if let Some(policy) = cfg.runtime_policy.as_mut() {
            policy.notes.push(format!("{}1024 tokens: test.", crate::runtime_fit::MICRO_BATCH_NOTE_PREFIX));
        }
        let mut attempts = vec![];
        let (active, notice) = load_with_fallback(cfg, true, Devices::Available, |cfg| {
            attempts.push((cfg.n_gpu_layers, cfg.micro_batch, cfg.load_without_mmap));
            async move {
                if cfg.load_without_mmap {
                    Err(InferenceError::Generation("llama_model_load: error loading model: unable to allocate CUDA_Host buffer".into()))
                } else {
                    Ok(cfg)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(attempts, [(-1, Some(1024), true), (-1, None, false)], "the GPU again, without the tuning, before the CPU");
        assert_eq!(active.n_gpu_layers, -1);
        assert!(notice.unwrap().contains("runtime's defaults"));
        assert!(!active.runtime_policy.unwrap().notes.iter().any(|note| note.starts_with("Micro-batch")));

        // Without tuning to drop, a GPU failure goes straight to the CPU as before.
        let mut attempts = 0;
        let result: Result<((), Option<String>), _> = load_with_fallback(configured(), true, Devices::Available, |cfg| {
            attempts += 1;
            async move {
                if cfg.n_gpu_layers == 0 {
                    Ok(())
                } else {
                    Err(InferenceError::Generation("CUDA error: out of memory".into()))
                }
            }
        })
        .await;
        assert_eq!(attempts, 2);
        assert!(result.unwrap().1.unwrap().contains("CPU"));
    }
}
