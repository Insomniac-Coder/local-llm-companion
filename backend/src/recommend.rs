//! Context recommendation engine (§§154–169).
//!
//! Goal (§190): maximum USEFUL context, not maximum possible. The engine
//! combines hardware, model/quantization, runtime config, live sessions,
//! and workload to recommend the largest context the machine sustains
//! reliably — with margins, rationale, and confidence, never a bare number.

use serde::{Deserialize, Serialize};

/// VRAM/RAM headroom never offered to any single session (§161).
pub const SAFETY_RESERVE: f64 = 0.15;
/// Compute buffers as a fraction of model weights (§160).
pub const COMPUTE_OVERHEAD: f64 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Workload {
    Chat,
    Docs,
    Coding,
    Agent,
    Vision,
}

impl Workload {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "docs" | "documents" | "long" => Self::Docs,
            "coding" | "code" | "codebase" => Self::Coding,
            "agent" => Self::Agent,
            "vision" | "image" | "images" => Self::Vision,
            _ => Self::Chat,
        }
    }

    /// Workload appetite multiplier on the affordable ceiling (§155, §167).
    /// Coding/agents want room; chat stays lean for responsiveness.
    fn appetite(&self) -> f64 {
        match self {
            Self::Chat => 0.5,
            Self::Docs => 0.75,
            Self::Coding => 1.0,
            Self::Agent => 1.0,
            Self::Vision => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecProfile {
    Efficient,
    Balanced,
    Maximum,
}

impl RecProfile {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "efficient" => Self::Efficient,
            "maximum" | "max" => Self::Maximum,
            _ => Self::Balanced,
        }
    }

    /// Fraction of the affordable ceiling this profile targets (§156).
    fn target(&self) -> f64 {
        match self {
            Self::Efficient => 0.4,
            Self::Balanced => 0.7,
            Self::Maximum => 1.0,
        }
    }
}

/// Bytes per parameter by quantization family (fit for recommendation, not
/// a loader — actual residency is measured at load, §160).
pub fn bytes_per_param(quant: &str) -> f64 {
    let q = quant.to_lowercase();
    if q.contains("f32") {
        4.0
    } else if q.contains("f16") || q.contains("bf16") {
        2.0
    } else if q.contains("q8") {
        1.05
    } else if q.contains("q6") {
        0.78
    } else if q.contains("q5") {
        0.68
    } else if q.contains("q4") {
        0.58
    } else if q.contains("q3") {
        0.42
    } else if q.contains("q2") || q.contains("iq") {
        0.34
    } else {
        0.6 // unknown quant: assume Q4-class, say so in rationale
    }
}

/// "14B" / "1.5B" / "8x7B" (MoE total) → billions of parameters.
pub fn parse_params_b(parameters: &str) -> Option<f64> {
    let p = parameters.trim().to_lowercase().replace('×', "x");
    let p = p.strip_suffix('b').unwrap_or(&p);
    // MoE "8x7B": total experts matter for VRAM, active for speed.
    if let Some((a, b)) = p.split_once('x') {
        return Some(a.trim().parse::<f64>().ok()? * b.trim().parse::<f64>().ok()?);
    }
    p.trim().parse::<f64>().ok()
}

/// Heuristic transformer shape from parameter count (calibrated against
/// Llama/Qwen dense checkpoints; MoE uses totals conservatively).
fn shape(params_b: f64) -> (usize, usize) {
    let layers = (24.0 + params_b * 1.7).clamp(24.0, 88.0).round() as usize;
    let hidden = ((params_b * 114.0).sqrt().round() as usize * 128).max(1024);
    (layers, hidden)
}

/// Approximate KV bytes/token for the managed f16 cache: K + V × layers ×
/// estimated GQA width × 2 bytes. Actual attention/recurrent architecture can
/// differ substantially; this is an advisory estimate, never measured usage.
pub fn kv_per_token(params_b: f64) -> f64 {
    let (layers, hidden) = shape(params_b);
    2.0 * layers as f64 * (hidden as f64 / 4.0) * 2.0
}

#[derive(Debug, Clone)]
pub struct RecInputs {
    pub params_b: f64,
    pub quant: String,
    pub train_ctx: u32,
    pub vram_gb: f64,
    pub ram_gb: f64,
    pub gpu_layers_full: bool,
    pub other_sessions: usize,
    pub workload: Workload,
    pub profile: RecProfile,
    pub image_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub ctx: u32,
    pub kv_gb: f64,
    pub total_gb: f64,
    /// supported | recommended | risky | unavailable (§159)
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub recommended_ctx: u32,
    pub profile: RecProfile,
    pub confidence: String,
    pub weights_gb: f64,
    pub kv_gb: f64,
    pub headroom_gb: f64,
    pub rationale: Vec<String>,
    pub candidates: Vec<Candidate>,
}

pub fn weights_gb(params_b: f64, quant: &str) -> f64 {
    params_b * 1e9 * bytes_per_param(quant) / 1_073_741_824.0
}

const STEPS: [u32; 6] = [4096, 8192, 16384, 32768, 65536, 131072];

pub fn recommend(inp: &RecInputs) -> Recommendation {
    let weights = weights_gb(inp.params_b, &inp.quant);
    let compute = weights * COMPUTE_OVERHEAD;
    // VRAM-first when offloading, else RAM; never offer the reserve (§161).
    let pool = if inp.gpu_layers_full && inp.vram_gb > 0.0 {
        inp.vram_gb
    } else {
        inp.ram_gb
    };
    let usable = (pool * (1.0 - SAFETY_RESERVE) - weights - compute).max(0.0);
    // Concurrent sessions share the pool (§160: never assume it all).
    let share = usable / (1 + inp.other_sessions) as f64;
    let kv_tok = kv_per_token(inp.params_b);
    // Vision sessions reserve image budget first (§166): ~1.5K tokens/image.
    let image_reserve_gb = inp.image_count as f64 * 1500.0 * kv_tok / 1_073_741_824.0;
    let for_kv = (share - image_reserve_gb).max(0.0);
    let affordable = (for_kv * 1_073_741_824.0 / kv_tok).floor() as u32;
    let target = (affordable as f64 * inp.workload.appetite() * inp.profile.target()) as u32;

    let mut candidates = vec![];
    for &ctx in &STEPS {
        let kv = ctx as f64 * kv_tok / 1_073_741_824.0;
        let total = weights + compute + kv;
        let verdict = if ctx > inp.train_ctx {
            "unavailable"
        } else if ctx as f64 <= target as f64 {
            "recommended"
        } else if (ctx as f64) <= affordable as f64 {
            "supported"
        } else if total <= pool {
            "risky"
        } else {
            "unavailable"
        };
        candidates.push(Candidate {
            ctx,
            kv_gb: (kv * 10.0).round() / 10.0,
            total_gb: (total * 10.0).round() / 10.0,
            verdict: verdict.into(),
        });
    }
    // Largest recommended step within training length, else smallest supported.
    let mut recommended = STEPS
        .iter()
        .filter(|c| **c <= inp.train_ctx && (**c as f64) <= target as f64)
        .max()
        .copied()
        .unwrap_or(4096);
    if recommended > inp.train_ctx {
        recommended = 4096;
    }
    let rec_kv = recommended as f64 * kv_tok / 1_073_741_824.0;
    let headroom = (pool - (weights + compute + rec_kv)).max(0.0);
    // Headroom alone cannot validate an inferred architecture or KV layout.
    let confidence = "Medium";
    let mut rationale = vec![
        format!(
            "Model weights ~{:.1} GB ({} @ {})",
            weights, inp.params_b, inp.quant
        ),
        format!(
            "KV cache ~{:.1} GB at {}K (f16 cache, estimated architecture)",
            rec_kv,
            recommended / 1024
        ),
        format!(
            "{} pool {:.1} GB with {:.0}% reserve{}",
            if inp.gpu_layers_full && inp.vram_gb > 0.0 {
                "VRAM"
            } else {
                "RAM"
            },
            pool,
            SAFETY_RESERVE * 100.0,
            if inp.other_sessions > 0 {
                format!(" shared across {} sessions", inp.other_sessions + 1)
            } else {
                String::new()
            },
        ),
        format!("Workload {:?} × {:?} profile", inp.workload, inp.profile),
        "Advisory estimate, not measured allocation. Actual attention/recurrent layout and runtime buffers can differ; confirm with a successful load and live readings.".into(),
    ];
    if inp.image_count > 0 {
        rationale.push(format!(
            "Reserved ~{:.1} GB for {} image(s)",
            image_reserve_gb, inp.image_count
        ));
    }
    if confidence == "Medium" {
        rationale.push(
            "Medium confidence: architecture is inferred, not calibrated for this exact model. Watch Resources after loading.".into(),
        );
    }

    Recommendation {
        recommended_ctx: recommended,
        profile: inp.profile,
        confidence: confidence.into(),
        weights_gb: (weights * 10.0).round() / 10.0,
        kv_gb: (rec_kv * 10.0).round() / 10.0,
        headroom_gb: (headroom * 10.0).round() / 10.0,
        rationale,
        candidates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> RecInputs {
        RecInputs {
            params_b: 14.0,
            quant: "Q4_K_M".into(),
            train_ctx: 32768,
            vram_gb: 12.0,
            ram_gb: 64.0,
            gpu_layers_full: true,
            other_sessions: 0,
            workload: Workload::Coding,
            profile: RecProfile::Balanced,
            image_count: 0,
        }
    }

    #[test]
    fn fourteen_b_q4_on_12gb_uses_f16_cache_headroom() {
        // f16 doubles KV byte cost versus the old, incorrectly assumed q8 cache.
        let r = recommend(&rig());
        assert_eq!(r.recommended_ctx, 4096, "{r:?}");
        assert!(r
            .candidates
            .iter()
            .any(|c| c.ctx == 32768 && c.verdict != "recommended"));
        assert!(r.rationale.len() >= 4);
    }

    #[test]
    fn efficient_profile_is_leaner_than_maximum() {
        let mut eff = rig();
        eff.profile = RecProfile::Efficient;
        let mut max = rig();
        max.profile = RecProfile::Maximum;
        assert!(recommend(&eff).recommended_ctx <= recommend(&max).recommended_ctx);
    }

    #[test]
    fn chat_appetite_below_coding() {
        let mut chat = rig();
        chat.workload = Workload::Chat;
        assert!(recommend(&chat).recommended_ctx <= recommend(&rig()).recommended_ctx);
    }

    #[test]
    fn unknown_quant_assumes_q4_class() {
        assert_eq!(bytes_per_param("MYSTERY"), 0.6);
        assert!(parse_params_b("8x7B") == Some(56.0));
        assert!(parse_params_b("1.5B") == Some(1.5));
    }

    #[test]
    fn tiny_model_gets_large_context() {
        let mut small = rig();
        small.params_b = 1.5;
        small.quant = "Q4_K_M".into();
        let r = recommend(&small);
        assert!(r.recommended_ctx >= 32768, "{r:?}");
        assert_eq!(
            r.confidence, "Medium",
            "headroom is not architecture calibration"
        );
        assert!(r.rationale.iter().any(|line| line.contains("f16 cache")));
    }

    #[test]
    fn f16_kv_estimate_accounts_for_two_bytes_per_component() {
        let (layers, hidden) = shape(8.0);
        assert_eq!(
            kv_per_token(8.0),
            2.0 * layers as f64 * (hidden as f64 / 4.0) * 2.0
        );
    }
}
