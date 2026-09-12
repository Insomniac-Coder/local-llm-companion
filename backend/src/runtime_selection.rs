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
    cfg.n_ctx = cfg.n_ctx.min(8192);
    cfg.n_batch = cfg.n_batch.min(128).min(cfg.n_ctx).max(1);
    if let Some(policy) = &mut cfg.runtime_policy {
        policy.gpu_layers = 0;
        policy.kv_offload = "off".into();
        policy.flash_attention = "off".into();
        policy.threads = 0;
        policy.effective_context = cfg.n_ctx;
        policy.batch_size = cfg.n_batch;
        policy.notes.push(format!("CPU mode selected automatically: {reason}. GPU, cache and projector offloading are disabled. Context is capped at 8192 and batch size at 128 for CPU operation; saved preferences are unchanged. Responses may be slower, and the model must still fit available system memory."));
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
    match launch(cfg.clone()).await {
        Ok(result) => Ok((result, None)),
        Err(first) if gpu_startup_failure(&first) => {
            tracing::warn!("GPU runtime startup failed; retrying this model once on CPU: {first}");
            match launch(cpu_configuration(cfg, "GPU initialization or allocation failed")).await {
                Ok(result) => Ok((result, Some("GPU startup failed; the model is now running on CPU. Responses may be slower.".into()))),
                Err(cpu) => Err(InferenceError::Generation(format!("Automatic GPU startup and CPU fallback both failed. CPU attempt: {cpu}. Original GPU attempt: {first}. Saved conversations were not changed."))),
            }
        }
        Err(error) => Err(error),
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
        assert_eq!(active.n_batch, 128);
        assert_eq!(cfg.n_ctx, 32768);
        assert_eq!(cfg.n_gpu_layers, -1);
        assert!(notice.unwrap().contains("CPU"));
        assert_eq!(active.runtime_policy.unwrap().effective_context, 8192);
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
}
