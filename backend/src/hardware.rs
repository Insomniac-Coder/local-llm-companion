//! Hardware detection (§11) + resource monitoring (§47–§48).

use serde::{Deserialize, Serialize};
use sysinfo::System;

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

pub fn detect() -> HardwareReport {
    let mut sys = System::new_all();
    sys.refresh_all();
    let cpu_model = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into());
    let logical = sys.cpus().len();
    let physical = sys.physical_core_count().unwrap_or(logical);
    let total_gb = sys.total_memory() as f64 / 1_073_741_824.0;
    let avail_gb = sys.available_memory() as f64 / 1_073_741_824.0;
    HardwareReport {
        cpu: CpuInfo {
            model: cpu_model,
            physical_cores: physical,
            logical_cores: logical,
        },
        ram: RamInfo {
            total_gb: (total_gb * 10.0).round() / 10.0,
            available_gb: (avail_gb * 10.0).round() / 10.0,
        },
        // GPU enumeration needs platform APIs (NVML/ADL/IOKit); Stage 11 wires it.
        // Until then report a CPU fallback so auto-config degrades gracefully (§4).
        gpus: vec![GpuInfo {
            vendor: "unknown".into(),
            model: "no GPU enumerated yet (Stage 11)".into(),
            vram_gb: 0.0,
            available_vram_gb: 0.0,
            backend: "cpu".into(),
        }],
        os: std::env::consts::OS.into(),
    }
}

pub fn snapshot(context_used: u32, context_limit: u32, tps: f32) -> ResourceSnapshot {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.refresh_cpu();
    let total_gb = sys.total_memory() as f64 / 1_073_741_824.0;
    let avail = sys.available_memory() as f64 / 1_073_741_824.0;
    // sysinfo 0.30 CPU usage needs a refresh interval; first call may be 0.
    let cpu = sys.global_cpu_info().cpu_usage();
    ResourceSnapshot {
        cpu_percent: cpu,
        ram_used_gb: ((total_gb - avail) * 10.0).round() / 10.0,
        ram_total_gb: (total_gb * 10.0).round() / 10.0,
        vram_used_gb: 0.0,
        vram_total_gb: 0.0,
        gpu_percent: 0.0,
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
        let s = snapshot(0, 0, 0.0);
        assert!(
            (0.5..=8192.0).contains(&s.ram_total_gb),
            "implausible snapshot RAM: {}",
            s.ram_total_gb
        );
    }
}
