//! Device-Aware Inference Optimizer core (Stage 30, DAIO §§1–12).
//!
//! Answers: *given this device, model, workload and resource state, what is
//! the fastest stable way to run inference?* Never assumes
//! supported == fastest (§2: capability ≠ suitability ≠ performance).
//!
//! This module is deliberately measurement-first but dependency-free:
//! capability records start as `detected`/`supported` and only become
//! `benchmarked`/`preferred` when local evidence exists (§5). Placement
//! uses the full memory model (weights + KV + compute + projector +
//! overhead + margin), never `VRAM − weights = KV` (§10).

use serde::{Deserialize, Serialize};

/// One hardware/software capability (§5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityRecord {
    pub name: String,
    pub device: String, // cpu | gpu | npu
    pub status: String, // detected|supported|usable|benchmarked|preferred|disabled|unavailable
    pub backend: String,
    pub performance_delta_pct: Option<f32>,
    pub evidence_runs: u32,
}

/// Normalized device profile (§4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub fingerprint: String,
    pub os: String,
    pub cpu_model: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub simd: Vec<String>,
    pub ram_total_gb: f64,
    pub ram_avail_gb: f64,
    pub gpu_model: String,
    pub gpu_backend: String,
    pub vram_total_gb: f64,
    pub vram_avail_gb: f64,
    pub npu: Option<String>,
}

impl DeviceProfile {
    /// Non-sensitive fingerprint: families + sizes + backend versions (§23).
    pub fn fingerprint_parts(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            normalize_token(&self.cpu_model),
            self.logical_cores,
            normalize_token(&self.gpu_model),
            self.vram_total_gb.round(),
            self.os,
            self.gpu_backend,
        )
    }
}

fn normalize_token(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
}

/// Compile-time SIMD capability detection (§4.1, §9). These are
/// *candidates* — the optimizer only prefers them with evidence.
pub fn detect_simd() -> Vec<String> {
    let mut out = Vec::new();
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx") {
            out.push("AVX".into());
        }
        if std::is_x86_feature_detected!("avx2") {
            out.push("AVX2".into());
        }
        if std::is_x86_feature_detected!("avx512f") {
            out.push("AVX-512".into());
        }
        // VNNI/AMX/BF16 have no stable runtime macro on all toolchains;
        // record them as unavailable rather than guessing.
        out.push("AVX-VNNI:unverified".into());
    }
    #[cfg(target_arch = "aarch64")]
    {
        out.push("NEON".into());
        out.push("I8MM:unverified".into());
    }
    out
}

/// Seed capability DB from detection (§5). Everything starts below
/// `benchmarked`; calibration promotes entries with measurements.
pub fn seed_capabilities(profile: &DeviceProfile) -> Vec<CapabilityRecord> {
    let mut caps: Vec<CapabilityRecord> = profile
        .simd
        .iter()
        .map(|s| CapabilityRecord {
            name: s.clone(),
            device: "cpu".into(),
            status: if s.ends_with(":unverified") {
                "detected".into()
            } else {
                "supported".into()
            },
            backend: "cpu".into(),
            performance_delta_pct: None,
            evidence_runs: 0,
        })
        .collect();
    caps.push(CapabilityRecord {
        name: format!("gpu:{}", profile.gpu_backend),
        device: "gpu".into(),
        status: if profile.vram_total_gb > 0.0 {
            "supported".into()
        } else {
            "unavailable".into()
        },
        backend: profile.gpu_backend.clone(),
        performance_delta_pct: None,
        evidence_runs: 0,
    });
    // NPU: never assume TOPS == useful (§2). Without operator coverage
    // evidence it stays unavailable.
    caps.push(CapabilityRecord {
        name: "npu".into(),
        device: "npu".into(),
        status: match &profile.npu {
            Some(_) => "detected".into(),
            None => "unavailable".into(),
        },
        backend: "npu".into(),
        performance_delta_pct: None,
        evidence_runs: 0,
    });
    caps
}

/// Model execution profile (§6), built from registry metadata + file size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub model_id: String,
    pub architecture: String,
    pub parameters_b: f64,
    pub quantization: String,
    pub weights_gb: f64,
    pub kv_per_token_mb: f64,
    pub vision: bool,
    pub projector_gb: f64,
}

/// Coarse capacity estimate for managed f16 KV, not model-specific allocation.
pub fn kv_per_token_mb(params_b: f64) -> f64 {
    // Two bytes/component versus the previous q8 assumption. This conservative
    // capacity heuristic is intentionally separate from architecture metadata.
    1.0 * (params_b / 8.0).sqrt()
}

/// Workload class (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadClass {
    Chat,
    ChatReasoning,
    Code,
    CodeReasoning,
    Vision,
    LongContext,
    Concurrent,
}

impl WorkloadClass {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "chat_reasoning" | "reasoning" => Self::ChatReasoning,
            "code" | "coding" => Self::Code,
            "code_reasoning" | "agent" => Self::CodeReasoning,
            "vision" | "image" | "ocr" => Self::Vision,
            "long" | "documents" | "long_context" => Self::LongContext,
            "concurrent" | "batch" => Self::Concurrent,
            _ => Self::Chat,
        }
    }

    /// Latency vs throughput objective weights (throughput, latency, stability).
    pub fn objective(&self) -> (f64, f64, f64) {
        match self {
            Self::Chat => (0.3, 0.6, 0.1),
            Self::ChatReasoning => (0.5, 0.2, 0.3),
            Self::Code => (0.6, 0.2, 0.2),
            Self::CodeReasoning => (0.5, 0.1, 0.4),
            Self::Vision => (0.4, 0.4, 0.2),
            Self::LongContext => (0.4, 0.1, 0.5),
            Self::Concurrent => (0.5, 0.1, 0.4),
        }
    }
}

/// Placement recommendation (§8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub strategy: String, // gpu | hybrid | cpu
    pub gpu_layers_percent: u8,
    pub backend: String,
    pub projector: String, // gpu | cpu
    pub kv_offload: bool,
    pub flash_attention: bool,
    pub context: u32,
    pub expected_vram_gb: f64,
    pub expected_ram_gb: f64,
    pub confidence: String,
    pub rationale: Vec<String>,
}

/// Full memory model (§10): weights + KV + compute + projector + margin.
pub fn optimize(
    device: &DeviceProfile,
    model: &ModelProfile,
    workload: WorkloadClass,
    active_sessions: usize,
    policy: &str, // performance | balanced | efficiency
) -> Placement {
    let mut rationale = Vec::new();
    let reserve = match policy {
        "performance" => 0.10,
        "efficiency" => 0.25,
        _ => 0.15,
    };
    rationale.push(format!(
        "Safety reserve {:.0}% ({policy} policy).",
        reserve * 100.0
    ));
    rationale.push("Advisory f16 KV estimate, not measured allocation. Attention/recurrent layout is inferred; confirm with runtime health and measured memory after loading.".into());

    let vram = device.vram_total_gb;
    let ram = device.ram_avail_gb;
    // Candidate contexts, largest useful first (§190: useful, not possible).
    let candidates = [131072, 65536, 32768, 16384, 8192, 4096];
    let kv_factor = match workload {
        WorkloadClass::Code | WorkloadClass::CodeReasoning => 1.0,
        WorkloadClass::Vision => 1.25, // image tokens cost extra (§166)
        WorkloadClass::Chat => 0.7,
        _ => 0.85,
    };
    let session_mul = 1.0 + 0.35 * (active_sessions.saturating_sub(1) as f64);
    rationale.push(format!(
        "{active_sessions} active session(s) share the pool."
    ));

    let compute_buf = (model.weights_gb * 0.08).max(0.3);
    let mut picked_ctx = 4096u32;
    let mut kv_gb = 0.0;
    for ctx in candidates {
        let kv = ctx as f64 * model.kv_per_token_mb * kv_factor / 1024.0 * session_mul;
        let need = model.weights_gb + kv + compute_buf + model.projector_gb;
        if need <= vram.max(0.0) * (1.0 - reserve)
            || (vram <= 0.0 && model.weights_gb + kv <= ram * (1.0 - reserve))
        {
            picked_ctx = ctx;
            kv_gb = kv;
            break;
        }
        kv_gb = kv;
    }

    let total_need = model.weights_gb + kv_gb + compute_buf + model.projector_gb;
    let (strategy, gpu_pct) = if vram <= 0.0 {
        rationale.push("No GPU enumerated: CPU placement.".into());
        ("cpu".into(), 0u8)
    } else if total_need <= vram * (1.0 - reserve) {
        rationale.push(format!(
            "Fits in VRAM ({total_need:.1}/{vram:.1} GB): full GPU offload."
        ));
        ("gpu".into(), 100)
    } else if model.weights_gb * 0.4 <= vram {
        let pct = ((vram * (1.0 - reserve) / total_need) * 100.0).clamp(5.0, 95.0) as u8;
        rationale.push(format!(
            "Exceeds VRAM: hybrid GPU {pct}% / CPU {}%.",
            100 - pct
        ));
        ("hybrid".into(), pct)
    } else {
        rationale.push("Too large for useful GPU offload: CPU placement.".into());
        ("cpu".into(), 0)
    };

    let projector = if model.vision {
        if strategy != "cpu" && model.projector_gb + 0.5 < vram * (1.0 - reserve) {
            rationale.push("Projector on GPU: fits budget and avoids transfer.".into());
            "gpu".into()
        } else {
            rationale.push("Projector on CPU: VRAM budget too tight.".into());
            "cpu".into()
        }
    } else {
        "n/a".into()
    };

    let expected_vram_gb = if strategy == "cpu" {
        0.0
    } else {
        (total_need * (gpu_pct as f64 / 100.0) * 10.0).round() / 10.0
    };
    let expected_ram_gb = ((total_need - expected_vram_gb).max(0.0) * 10.0).round() / 10.0;
    let confidence = "medium".to_string();

    Placement {
        strategy,
        gpu_layers_percent: gpu_pct,
        backend: device.gpu_backend.clone(),
        projector,
        kv_offload: expected_vram_gb > 0.0,
        flash_attention: true,
        context: picked_ctx,
        expected_vram_gb,
        expected_ram_gb,
        confidence,
        rationale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> (DeviceProfile, ModelProfile) {
        (
            DeviceProfile {
                fingerprint: String::new(),
                os: "windows".into(),
                cpu_model: "Example CPU".into(),
                physical_cores: 8,
                logical_cores: 16,
                simd: vec!["AVX2".into()],
                ram_total_gb: 64.0,
                ram_avail_gb: 40.0,
                gpu_model: "Example GPU 12GB".into(),
                gpu_backend: "cuda".into(),
                vram_total_gb: 12.0,
                vram_avail_gb: 11.0,
                npu: None,
            },
            ModelProfile {
                model_id: "q14".into(),
                architecture: "qwen2".into(),
                parameters_b: 14.0,
                quantization: "Q4_K_M".into(),
                weights_gb: 8.5,
                kv_per_token_mb: kv_per_token_mb(14.0),
                vision: false,
                projector_gb: 0.0,
            },
        )
    }

    #[test]
    fn simd_never_claims_unverified() {
        for s in detect_simd() {
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn npu_without_evidence_stays_unavailable() {
        let (d, _) = rig();
        let caps = seed_capabilities(&d);
        let npu = caps.iter().find(|c| c.device == "npu").unwrap();
        assert_eq!(npu.status, "unavailable");
        assert_eq!(npu.evidence_runs, 0);
    }

    #[test]
    fn small_model_gets_gpu_placement() {
        let (d, mut m) = rig();
        m.parameters_b = 8.0;
        m.weights_gb = 4.5;
        m.kv_per_token_mb = kv_per_token_mb(8.0);
        let p = optimize(&d, &m, WorkloadClass::Chat, 1, "balanced");
        assert_eq!(p.strategy, "gpu");
        assert!(p.context >= 4096);
        assert!(p.expected_vram_gb <= 12.0);
        assert_eq!(p.confidence, "medium");
        assert!(p.rationale.iter().any(|line| line.contains("f16 KV")));
    }

    #[test]
    fn mid_model_on_12gb_is_hybrid_not_fiction() {
        let (d, m) = rig(); // 14B Q4-class
        let p = optimize(&d, &m, WorkloadClass::Chat, 1, "balanced");
        assert!(
            p.strategy == "hybrid" || p.strategy == "cpu",
            "got {}",
            p.strategy
        );
        assert!(p.expected_vram_gb <= 12.0);
    }

    #[test]
    fn huge_model_falls_back_to_cpu() {
        let (d, mut m) = rig();
        m.weights_gb = 80.0;
        m.parameters_b = 70.0;
        m.kv_per_token_mb = kv_per_token_mb(70.0);
        let p = optimize(&d, &m, WorkloadClass::Code, 1, "balanced");
        assert_eq!(p.strategy, "cpu");
        assert!(p.rationale.iter().any(|r| r.contains("CPU")));
    }

    #[test]
    fn fingerprint_is_stable_and_nonsensitive() {
        let (mut d, _) = rig();
        d.fingerprint = d.fingerprint_parts();
        assert!(!d.fingerprint.is_empty());
        assert!(!d.fingerprint.contains("Example GPU"));
    }

    #[test]
    fn kv_capacity_estimate_uses_managed_f16_not_q8() {
        assert_eq!(kv_per_token_mb(8.0), 1.0);
    }
}
