//! Hardware detection (§11) + resource monitoring (§47–§48).
//!
//! Static facts (CPU model, core counts, RAM size, OS) are read once; the
//! dynamic readings come from a memory-only refresh. The previous version
//! built a full `System::new_all()` (every process on the machine, with
//! command lines) on each call, and several request handlers call this.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub model: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamInfo {
    pub total_gb: f64,
    pub available_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub vendor: String,
    pub model: String,
    pub vram_gb: f64,
    pub available_vram_gb: f64,
    /// llama.cpp backend: cuda | vulkan | metal | hip | cpu
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareReport {
    pub cpu: CpuInfo,
    pub ram: RamInfo,
    pub gpus: Vec<GpuInfo>,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_percent: f32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub vram_used_gb: f64,
    pub vram_total_gb: f64,
    pub gpu_percent: f32,
    pub context_used: u32,
    pub context_limit: u32,
    pub tokens_per_sec: f32,
}

struct StaticInfo {
    cpu: CpuInfo,
    total_gb: f64,
    os: String,
}

fn static_info() -> &'static StaticInfo {
    static INFO: OnceLock<StaticInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::new())
                .with_memory(MemoryRefreshKind::new().with_ram()),
        );
        let cpu_model = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown CPU".into());
        let logical = sys.cpus().len();
        let physical = sys.physical_core_count().unwrap_or(logical);
        StaticInfo {
            cpu: CpuInfo {
                model: cpu_model,
                physical_cores: physical,
                logical_cores: logical,
            },
            total_gb: (sys.total_memory() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0,
            os: std::env::consts::OS.into(),
        }
    })
}

fn available_ram_gb() -> f64 {
    let sys = System::new_with_specifics(
        RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram()),
    );
    (sys.available_memory() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0
}

pub fn detect() -> HardwareReport {
    let info = static_info();
    HardwareReport {
        cpu: info.cpu.clone(),
        ram: RamInfo {
            total_gb: info.total_gb,
            available_gb: available_ram_gb(),
        },
        // GPU enumeration needs platform APIs (NVML/ADL/IOKit); the resource
        // sampler supplies measured VRAM where a driver reports it, and the
        // model runtime's own device list decides placement at load.
        gpus: vec![GpuInfo {
            vendor: "unknown".into(),
            model: "not enumerated (runtime device list decides placement)".into(),
            vram_gb: 0.0,
            available_vram_gb: 0.0,
            backend: "cpu".into(),
        }],
        os: info.os.clone(),
    }
}

/// Point-in-time readings. CPU and GPU utilisation come from the background
/// sampler's latest sample (a one-shot CPU reading is meaningless without a
/// prior interval); memory is read fresh.
pub fn snapshot(
    latest: Option<&crate::metrics::Sample>,
    context_used: u32,
    context_limit: u32,
    tps: f32,
) -> ResourceSnapshot {
    let info = static_info();
    let avail = available_ram_gb();
    ResourceSnapshot {
        cpu_percent: latest.map(|s| s.cpu_pct).unwrap_or(0.0),
        ram_used_gb: ((info.total_gb - avail).max(0.0) * 10.0).round() / 10.0,
        ram_total_gb: info.total_gb,
        vram_used_gb: latest.and_then(|s| s.vram_used_gb).unwrap_or(0.0),
        vram_total_gb: latest.and_then(|s| s.vram_total_gb).unwrap_or(0.0),
        gpu_percent: latest.and_then(|s| s.gpu_pct).unwrap_or(0.0),
        context_used,
        context_limit,
        tokens_per_sec: tps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_sane_ram() {
        let h = detect();
        assert!(h.ram.total_gb > 0.0);
        assert!(!h.cpu.model.is_empty());
        assert!(h.cpu.logical_cores > 0);
    }

    #[test]
    fn ram_units_are_gigabytes_not_megabytes() {
        // sysinfo 0.30 reports BYTES; a 0.5 GB–8 TB window catches /1024 slips.
        let h = detect();
        assert!(
            (0.5..=8192.0).contains(&h.ram.total_gb),
            "implausible RAM total: {} (unit bug?)",
            h.ram.total_gb
        );
        let s = snapshot(None, 0, 0, 0.0);
        assert!(
            (0.5..=8192.0).contains(&s.ram_total_gb),
            "implausible snapshot RAM: {}",
            s.ram_total_gb
        );
        assert!(s.ram_used_gb <= s.ram_total_gb);
    }

    #[test]
    fn repeated_detection_is_cheap_and_stable() {
        let first = detect();
        let start = std::time::Instant::now();
        for _ in 0..50 {
            let again = detect();
            assert_eq!(again.cpu.model, first.cpu.model);
            assert_eq!(again.ram.total_gb, first.ram.total_gb);
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "50 detections took {:?}; static facts must be cached",
            start.elapsed()
        );
    }
}
