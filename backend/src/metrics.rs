//! Resource monitoring (§§129–148): sampler, history, alerts, attribution.
//!
//! A background task samples CPU/RAM (sysinfo) and GPU (nvidia-smi when
//! present) every 2 s into a ring buffer. Readings describe the whole machine,
//! not an individual process or session. Active-session counts are activity
//! metadata and must not be mistaken for measured per-session attribution.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SAMPLE_SECS: u64 = 2;
pub const CAPACITY: usize = 1800; // 1 hour at 2 s

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub ts: u64,
    pub cpu_pct: f32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub gpu_pct: Option<f32>,
    pub vram_used_gb: Option<f64>,
    pub vram_total_gb: Option<f64>,
    pub gpu_temp_c: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub active_sessions: usize,
}

#[derive(Debug, Clone)]
pub struct GpuReading {
    pub util_pct: f32,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    /// Marketing name of the measured card, so the UI can say which GPU the
    /// readings belong to. Absent on drivers that do not report it.
    pub name: Option<String>,
}

/// Parse one `nvidia-smi --format=csv,noheader,nounits` line:
/// `util, mem_used_mib, mem_total_mib, temp, power, name`
fn parse_smi_line(line: &str) -> Option<GpuReading> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    let num = |s: &str| s.split_whitespace().next().unwrap_or(s).parse::<f64>().ok();
    Some(GpuReading {
        util_pct: num(parts[0])? as f32,
        mem_used_gb: num(parts[1])? / 1024.0,
        mem_total_gb: num(parts[2])? / 1024.0,
        temp_c: parts.get(3).and_then(|s| num(s)).map(|v| v as f32),
        power_w: parts.get(4).and_then(|s| num(s)).map(|v| v as f32),
        // A name is the last column; rejoin in case a driver ever reports one
        // containing a comma. "[N/A]"-style placeholders are not names.
        name: (parts.len() > 5)
            .then(|| parts[5..].join(", "))
            .filter(|name| !name.is_empty() && !name.starts_with('[')),
    })
}

fn read_nvidia_smi() -> Option<GpuReading> {
    let mut command = std::process::Command::new("nvidia-smi");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: sampling must not flash a console.
    }
    let out = command
        .args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,name",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Multi-GPU: report the busiest card (single-GPU laptop honesty first).
    text.lines().filter_map(parse_smi_line).max_by(|a, b| {
        a.util_pct
            .partial_cmp(&b.util_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Best-effort VRAM totals for the recommendation engine: nvidia-smi when
/// present, since sysinfo cannot see GPUs. Returns (used_gb, total_gb).
pub fn vram_info() -> Option<(f64, f64)> {
    read_nvidia_smi().map(|g| (g.mem_used_gb, g.mem_total_gb))
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default)]
pub struct MetricsLog {
    samples: VecDeque<Sample>,
    smi_missing: bool,
    /// Name of the card the latest GPU readings came from. Kept once here
    /// rather than in every sample, which would repeat it in each history row.
    gpu_name: Option<String>,
}

impl MetricsLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, s: Sample) {
        if self.samples.len() >= CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(s);
    }

    pub fn latest(&self) -> Option<Sample> {
        self.samples.back().cloned()
    }

    pub fn set_gpu_name(&mut self, name: Option<String>) {
        if name.is_some() {
            self.gpu_name = name;
        }
    }

    pub fn gpu_name(&self) -> Option<&str> {
        self.gpu_name.as_deref()
    }

    /// Downsampled window for graphs (§134): at most ~120 points.
    pub fn window(&self, secs: u64) -> Vec<Sample> {
        let now = now_ts();
        let from = now.saturating_sub(secs);
        let pts: Vec<Sample> = self
            .samples
            .iter()
            .filter(|s| s.ts >= from)
            .cloned()
            .collect();
        if pts.len() <= 120 {
            return pts;
        }
        // Inclusive endpoints: integer step_by could return 239 points and
        // omit the newest reading, making the plot disagree with its readout.
        (0..120)
            .map(|index| pts[index * (pts.len() - 1) / 119].clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

pub type SharedLog = Arc<Mutex<MetricsLog>>;

/// A standalone measurement needs two CPU refreshes separated in time.
/// The background sampler below reuses its System instead of paying this
/// warm-up (and rebuilding the process inventory) every two seconds.
pub async fn sample_once(active_sessions: usize) -> Sample {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_usage();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    tokio::task::spawn_blocking(move || collect_sample(&mut sys, active_sessions).0)
        .await
        .expect("resource sampling task")
}

/// One reading plus the name of the GPU it measured, when known.
fn collect_sample(sys: &mut sysinfo::System, active_sessions: usize) -> (Sample, Option<String>) {
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    let gpu = read_nvidia_smi();
    let total_gb = sys.total_memory() as f64 / 1_073_741_824.0;
    let avail = sys.available_memory() as f64 / 1_073_741_824.0;
    let sample = Sample {
        ts: now_ts(),
        cpu_pct: sys.global_cpu_info().cpu_usage(),
        ram_used_gb: ((total_gb - avail) * 10.0).round() / 10.0,
        ram_total_gb: (total_gb * 10.0).round() / 10.0,
        gpu_pct: gpu.as_ref().map(|g| g.util_pct),
        vram_used_gb: gpu.as_ref().map(|g| (g.mem_used_gb * 10.0).round() / 10.0),
        vram_total_gb: gpu.as_ref().map(|g| (g.mem_total_gb * 10.0).round() / 10.0),
        gpu_temp_c: gpu.as_ref().and_then(|g| g.temp_c),
        gpu_power_w: gpu.as_ref().and_then(|g| g.power_w),
        active_sessions,
    };
    (sample, gpu.and_then(|g| g.name))
}

/// Background sampler. `active` counts live generations + agent runs.
pub async fn sampler(log: SharedLog, active: Arc<dyn Fn() -> usize + Send + Sync>) {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_usage();
    let mut tick = tokio::time::interval(Duration::from_secs(SAMPLE_SECS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // Prime the interval; first reading follows a real CPU interval.
    loop {
        tick.tick().await;
        let active_sessions = active();
        // Both sysinfo collection and the optional GPU subprocess stay off the
        // async runtime. Move the same measuring state back each iteration.
        let sampled = tokio::task::spawn_blocking(move || {
            let (sample, gpu_name) = collect_sample(&mut sys, active_sessions);
            (sys, sample, gpu_name)
        })
        .await;
        let (next_sys, s, gpu_name) = match sampled {
            Ok(sampled) => sampled,
            Err(error) => {
                tracing::warn!(%error, "resource sampler failed; reinitializing CPU counters");
                sys = sysinfo::System::new();
                sys.refresh_cpu_usage();
                continue;
            }
        };
        sys = next_sys;
        let mut guard = log.lock().expect("lock");
        guard.set_gpu_name(gpu_name);
        guard.push(s);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub level: String, // warning | critical
    pub title: String,
    pub detail: String,
    pub suggestions: Vec<String>,
}

/// Actionable alerts (§136) from the latest sample.
pub fn alerts_for(latest: Option<&Sample>) -> Vec<Alert> {
    let mut out = vec![];
    let Some(s) = latest else {
        return out;
    };
    if let (Some(used), Some(total)) = (s.vram_used_gb, s.vram_total_gb) {
        if total > 0.0 && used / total > 0.9 {
            out.push(Alert {
                level: "warning".into(),
                title: "VRAM pressure".into(),
                detail: format!(
                    "{used:.1} / {total:.1} GB — the next context allocation may fail."
                ),
                suggestions: vec![
                    "Reduce context size".into(),
                    "Reduce GPU layers".into(),
                    "Stop idle inference (`Stop` in System)".into(),
                ],
            });
        }
    }
    if s.ram_total_gb > 0.0 && s.ram_used_gb / s.ram_total_gb > 0.92 {
        out.push(Alert {
            level: "warning".into(),
            title: "Memory pressure".into(),
            detail: format!("{:.1} / {:.1} GB used.", s.ram_used_gb, s.ram_total_gb),
            suggestions: vec!["Close unused sessions".into(), "Reduce context size".into()],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_smi_csv() {
        let r = parse_smi_line("67, 10301, 12226, 71, 95.50").unwrap();
        assert_eq!(r.util_pct, 67.0);
        assert!((r.mem_used_gb - 10.06).abs() < 0.01);
        assert!((r.mem_total_gb - 11.94).abs() < 0.01);
        assert_eq!(r.temp_c, Some(71.0));
        assert_eq!(r.name, None);
        assert!(parse_smi_line("garbage").is_none());
        assert!(parse_smi_line("1, 2").is_none());
    }

    #[test]
    fn parses_gpu_name_column() {
        let r = parse_smi_line("12, 2048, 12226, 55, 20.1, NVIDIA Example Laptop GPU").unwrap();
        assert_eq!(r.name.as_deref(), Some("NVIDIA Example Laptop GPU"));
        let unreported = parse_smi_line("12, 2048, 12226, 55, 20.1, [N/A]").unwrap();
        assert_eq!(unreported.name, None);
    }

    #[test]
    fn keeps_last_known_gpu_name() {
        let mut log = MetricsLog::new();
        assert_eq!(log.gpu_name(), None);
        log.set_gpu_name(Some("Card".into()));
        log.set_gpu_name(None);
        assert_eq!(log.gpu_name(), Some("Card"));
    }

    #[test]
    fn ring_caps_and_windows() {
        let mut log = MetricsLog::new();
        for i in 0..CAPACITY + 50 {
            log.push(Sample {
                ts: i as u64,
                cpu_pct: 0.0,
                ram_used_gb: 0.0,
                ram_total_gb: 64.0,
                gpu_pct: None,
                vram_used_gb: None,
                vram_total_gb: None,
                gpu_temp_c: None,
                gpu_power_w: None,
                active_sessions: 0,
            });
        }
        assert_eq!(log.len(), CAPACITY);
        assert!(log.window(60).len() <= 61);
    }

    #[test]
    fn downsampling_is_bounded_and_keeps_both_endpoints() {
        for count in [120, 121, 239, 240, 1799] {
            let mut log = MetricsLog::new();
            let now = now_ts();
            for index in 0..count {
                log.push(Sample {
                    ts: now - (count - index - 1) as u64,
                    cpu_pct: (index % 100) as f32,
                    ram_used_gb: 1.0,
                    ram_total_gb: 8.0,
                    gpu_pct: None,
                    vram_used_gb: None,
                    vram_total_gb: None,
                    gpu_temp_c: None,
                    gpu_power_w: None,
                    active_sessions: 0,
                });
            }
            let samples = log.window(3600);
            assert_eq!(samples.len(), count.min(120));
            assert_eq!(samples.first().unwrap().ts, now - count as u64 + 1);
            assert_eq!(samples.last().unwrap().ts, now);
            assert!(samples.windows(2).all(|pair| pair[0].ts < pair[1].ts));
        }
    }

    #[test]
    fn vram_alert_threshold() {
        let s = Sample {
            ts: 0,
            cpu_pct: 0.0,
            ram_used_gb: 10.0,
            ram_total_gb: 64.0,
            gpu_pct: Some(90.0),
            vram_used_gb: Some(11.5),
            vram_total_gb: Some(12.0),
            gpu_temp_c: None,
            gpu_power_w: None,
            active_sessions: 1,
        };
        let a = alerts_for(Some(&s));
        assert_eq!(a.len(), 1);
        assert!(a[0].suggestions.iter().any(|x| x.contains("context")));
        assert!(alerts_for(None).is_empty());
    }
}
