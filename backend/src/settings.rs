//! Settings (§49), profiles (§50), presets (§51).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub theme: String,
    pub default_model: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceSettings {
    pub context_size: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub repeat_penalty: f32,
    pub batch_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareSettings {
    pub cpu_threads: u32,
    /// Manual mode: prompt-processing threads; 0 uses `cpu_threads`.
    #[serde(default)]
    pub threads_batch: u32,
    /// Manual mode: `--poll` (0 sleeps between operations, 50 spin-waits).
    #[serde(default)]
    pub poll: Option<u8>,
    /// Manual mode: `--prio` (-1 low, 0 normal).
    #[serde(default)]
    pub priority: Option<i8>,
    pub gpu_backend: String,
    pub gpu_layers: i32,
    pub flash_attention: bool,
    pub kv_cache_gpu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub command_timeout_secs: u64,
    pub autonomous_enabled: bool,
    /// ask | accept_edits | plan | auto (`permissions::PERMISSION_MODES`).
    /// `autonomous_enabled` follows it ("auto"), for code that still reads the
    /// switch; a settings file from before the modes reads its mode from it.
    #[serde(default)]
    pub permission_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub allowed_dirs: Vec<String>,
    pub blocked_dirs: Vec<String>,
    pub network: String, // disabled | ask | selected
}

/// Stage 23 appearance section (§30): theme lives here alongside the
/// frontend-local override; startup + density stay explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,   // dark | light | system
    pub density: String, // comfortable | compact
    pub reduce_motion: bool,
}

/// Stage 23 workspace defaults (§27, §67).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub default_dir: String,
    pub confirm_outside_copy: bool,
}

/// Stage 23 memory policy (§§82–88).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySettings {
    /// automatic | off. The legacy "ask" was never implemented and reads as
    /// automatic (see `AppSettings::normalized`).
    pub auto_compact: String,
    pub compaction_keep_turns: usize,
    pub share_across_modes: bool,
    /// Compact when this share of the usable context is in use: the model's
    /// window minus the room kept for its next reply. Checked between steps
    /// of an agent run and before a chat reply, never during either.
    #[serde(default = "default_compact_at_pct")]
    pub compact_at_pct: u8,
}

fn default_compact_at_pct() -> u8 {
    90
}

impl MemorySettings {
    pub fn auto_compaction(&self) -> bool {
        self.auto_compact != "off"
    }

    /// The configured threshold, clamped to the range settings validation
    /// accepts, so a hand-edited value can never disable or thrash compaction.
    pub fn compact_threshold_pct(&self) -> u32 {
        u32::from(self.compact_at_pct).clamp(50, 98)
    }
}

impl AppSettings {
    /// Values written by older versions, mapped to what they mean now.
    pub fn normalized(mut self) -> Self {
        if self.memory.auto_compact == "ask" {
            self.memory.auto_compact = "automatic".into();
        }
        // The Performance mode replaced a single automatic/manual switch. A
        // save without a mode keeps meaning what its switch said; afterwards
        // the switch follows the mode, for code that still reads it.
        match self.runtime.mode.as_str() {
            "auto" | "fastest" | "balanced" | "light" | "manual" => {}
            _ => self.runtime.mode = if self.runtime_auto { "auto" } else { "manual" }.into(),
        }
        self.runtime_auto = self.runtime.mode != "manual";
        if !crate::permissions::PERMISSION_MODES.contains(&self.agent.permission_mode.as_str()) {
            self.agent.permission_mode = if self.agent.autonomous_enabled { "auto" } else { "ask" }.into();
        }
        self.agent.autonomous_enabled = self.agent.permission_mode == "auto";
        self
    }

    /// A save measured against the settings it replaces. A client that knows
    /// only the old automatic/manual switch sends back the mode it read,
    /// unchanged, with the switch flipped; the switch is then what the user
    /// changed, so the mode follows it instead of silently reverting it.
    pub fn reconciled_with(mut self, previous: &AppSettings) -> Self {
        if self.runtime_auto != previous.runtime_auto && self.runtime.mode == previous.runtime.mode {
            self.runtime.mode = if self.runtime_auto { "auto" } else { "manual" }.into();
        }
        // The same for a client that only knows the Auto switch.
        if self.agent.autonomous_enabled != previous.agent.autonomous_enabled
            && self.agent.permission_mode == previous.agent.permission_mode
        {
            self.agent.permission_mode = if self.agent.autonomous_enabled { "auto" } else { "ask" }.into();
        }
        self.normalized()
    }
}

/// Stage 23 files + documents (§§67–69, §§36–38).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesSettings {
    pub max_attach_mb: usize,
    pub max_image_mb: usize,
    pub ocr_enabled: bool,
}

/// Stage 23 keyboard shortcuts (§143).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardSettings {
    pub next_mode: String,
    pub command_palette: String,
}

/// Stage 23 privacy (§87).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacySettings {
    pub telemetry: bool,
    pub log_redaction: bool,
    /// Keep each model request and its output (capped) for diagnosis; they
    /// are included in conversation exports. On by default (owner decision).
    #[serde(default = "default_true")]
    pub record_model_requests: bool,
}

fn default_true() -> bool {
    true
}

/// Stage 23 network policy (§56).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub policy: String, // disabled | ask | selected
    pub trusted_hosts: Vec<String>,
}

/// Stage 23 advanced inference (§90, §169).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedSettings {
    pub kv_cache_type: String,
    pub kv_offload: String, // auto | on | off
    pub microbatch: u32,
    pub parallel_sessions: u32,
    pub server_port: u16,
}

/// Stage 23 diagnostics (§§51, 80).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSettings {
    pub log_level: String, // normal | verbose | debug
    pub show_generation_speed: bool,
    pub show_detailed_metrics: bool,
}
/// Stage 11 reasoning preference (§§110–115).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningSettings {
    /// Default when a message/conversation sets no override.
    pub default_on: bool,
    /// Budget: automatic | low | medium | high (§112, default Automatic).
    pub budget: String,
}

/// Stage 11 web-search preference (§§124, 128). Disabled unless the user
/// explicitly enables it per message (§116–117).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSettings {
    /// duckduckgo | brave | custom
    pub provider: String,
    pub brave_key: String,
    pub custom_url: String,
    pub max_results: usize,
    pub timeout_secs: u64,
    /// ask | allow | deny — search use inside autonomous agent runs.
    pub autonomous: String,
}

impl Default for ReasoningSettings {
    fn default() -> Self {
        Self {
            default_on: false,
            budget: "automatic".into(),
        }
    }
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            provider: "duckduckgo".into(),
            brave_key: String::new(),
            custom_url: String::new(),
            max_results: 5,
            timeout_secs: 15,
            autonomous: "ask".into(),
        }
    }
}

/// Runtime tuning that applies in both automatic and manual hardware modes.
/// Each option maps to one validated llama-server flag; see docs/PERFORMANCE.md
/// for the measurements behind the defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeSettings {
    /// "auto" drafts from the context (n-gram, lossless); "off" disables it.
    pub speculative: String,
    /// "f16" (compatibility default) or "q8_0" (half the cache memory).
    pub kv_cache: String,
    /// Reuse unchanged KV chunks after a prompt diverges (agent loops).
    pub cache_reuse: bool,
    /// "fit" (default) lets the loader shrink the context so the model stays
    /// on the GPU; "requested" keeps the saved context size even when that
    /// pushes layers onto the CPU. The saved size is never silently ignored:
    /// this decides which of the two the user meant.
    #[serde(default = "default_context_fit")]
    pub context_fit: String,
    /// auto | fastest | balanced | light | manual. The three profiles apply a
    /// model's own calibration; a model without one runs as auto. Older saves
    /// have no mode: `normalized` derives it from `runtime_auto`.
    #[serde(default)]
    pub mode: String,
}

impl RuntimeSettings {
    pub fn profile_name(&self) -> Option<&str> {
        matches!(self.mode.as_str(), "fastest" | "balanced" | "light").then_some(self.mode.as_str())
    }
}

fn default_context_fit() -> String {
    "fit".into()
}

impl RuntimeSettings {
    /// The user asked for their context size to stand as written.
    pub fn keeps_requested_context(&self) -> bool {
        self.context_fit == "requested"
    }
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            speculative: "auto".into(),
            kv_cache: "f16".into(),
            cache_reuse: true,
            context_fit: default_context_fit(),
            mode: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Managed loading defaults. Legacy inactive cache preferences remain stored,
    /// but are never reinterpreted as an explicit runtime override.
    #[serde(default = "default_runtime_auto")]
    pub runtime_auto: bool,
    #[serde(default)]
    pub runtime: RuntimeSettings,
    pub general: GeneralSettings,
    pub inference: InferenceSettings,
    pub hardware: HardwareSettings,
    pub agent: AgentSettings,
    pub security: SecuritySettings,
    pub reasoning: ReasoningSettings,
    pub search: SearchSettings,
    pub output_dir: String,
    /// Stage 23 sections (§30). All defaulted so old settings JSON still loads.
    #[serde(default = "default_appearance")]
    pub appearance: AppearanceSettings,
    #[serde(default = "default_workspace_settings")]
    pub workspace: WorkspaceSettings,
    #[serde(default = "default_memory_settings")]
    pub memory: MemorySettings,
    #[serde(default = "default_files_settings")]
    pub files: FilesSettings,
    #[serde(default = "default_keyboard_settings")]
    pub keyboard: KeyboardSettings,
    #[serde(default = "default_privacy_settings")]
    pub privacy: PrivacySettings,
    #[serde(default = "default_network_settings")]
    pub network: NetworkSettings,
    #[serde(default = "default_advanced_settings")]
    pub advanced: AdvancedSettings,
    #[serde(default = "default_diagnostics_settings")]
    pub diagnostics: DiagnosticsSettings,
}

fn default_runtime_auto() -> bool {
    true
}

fn default_appearance() -> AppearanceSettings {
    AppearanceSettings {
        theme: "dark".into(),
        density: "comfortable".into(),
        reduce_motion: false,
    }
}
fn default_workspace_settings() -> WorkspaceSettings {
    WorkspaceSettings {
        default_dir: "".into(),
        confirm_outside_copy: true,
    }
}
fn default_memory_settings() -> MemorySettings {
    MemorySettings {
        auto_compact: "automatic".into(),
        compaction_keep_turns: 10,
        share_across_modes: false,
        compact_at_pct: default_compact_at_pct(),
    }
}
fn default_files_settings() -> FilesSettings {
    FilesSettings {
        max_attach_mb: 5,
        max_image_mb: 8,
        ocr_enabled: false,
    }
}
fn default_keyboard_settings() -> KeyboardSettings {
    KeyboardSettings {
        next_mode: "ctrl+tab".into(),
        command_palette: "ctrl+k".into(),
    }
}
fn default_privacy_settings() -> PrivacySettings {
    PrivacySettings {
        telemetry: true,
        log_redaction: true,
        record_model_requests: true,
    }
}
fn default_network_settings() -> NetworkSettings {
    NetworkSettings {
        policy: "disabled".into(),
        trusted_hosts: vec![],
    }
}
fn default_advanced_settings() -> AdvancedSettings {
    AdvancedSettings {
        kv_cache_type: "Q8_0".into(),
        kv_offload: "auto".into(),
        microbatch: 0,
        parallel_sessions: 1,
        server_port: 3877,
    }
}
fn default_diagnostics_settings() -> DiagnosticsSettings {
    DiagnosticsSettings {
        log_level: "normal".into(),
        show_generation_speed: true,
        show_detailed_metrics: false,
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            runtime_auto: true,
            runtime: RuntimeSettings::default(),
            general: GeneralSettings {
                theme: "dark".into(),
                default_model: "".into(),
                language: "en".into(),
            },
            inference: InferenceSettings {
                context_size: 32768,
                temperature: 0.7,
                top_p: 0.9,
                top_k: 40,
                repeat_penalty: 1.1,
                batch_size: 512,
            },
            hardware: HardwareSettings {
                // The detected physical core count, which measured fastest
                // for both prompt and generation on the audit machine, rather
                // than a number chosen for one processor.
                cpu_threads: crate::cpu_topology::detect()
                    .map(|topology| topology.physical_count() as u32)
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map(|n| n.get() as u32)
                            .unwrap_or(1)
                    }),
                threads_batch: 0,
                poll: None,
                priority: None,
                gpu_backend: "auto".into(),
                gpu_layers: -1,
                flash_attention: true,
                kv_cache_gpu: true,
            },
            agent: AgentSettings {
                command_timeout_secs: 120,
                autonomous_enabled: false,
                permission_mode: "ask".into(),
            },
            security: SecuritySettings {
                allowed_dirs: vec![],
                blocked_dirs: vec![],
                network: "disabled".into(),
            },
            reasoning: ReasoningSettings::default(),
            search: SearchSettings::default(),
            output_dir: "artifacts".into(),
            appearance: default_appearance(),
            workspace: default_workspace_settings(),
            memory: default_memory_settings(),
            files: default_files_settings(),
            keyboard: default_keyboard_settings(),
            privacy: default_privacy_settings(),
            network: default_network_settings(),
            advanced: default_advanced_settings(),
            diagnostics: default_diagnostics_settings(),
        }
    }
}

/// §51 presets map to sensible llama.cpp configs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Fast,
    Balanced,
    Quality,
    Maximum,
    Custom,
}

pub fn apply_preset(s: &mut AppSettings, p: Preset) {
    match p {
        Preset::Fast => {
            s.inference.context_size = 8192;
            s.inference.batch_size = 256;
            s.hardware.gpu_layers = 20;
        }
        Preset::Balanced => {
            s.inference.context_size = 32768;
            s.inference.batch_size = 512;
            s.hardware.gpu_layers = -1;
        }
        Preset::Quality => {
            s.inference.context_size = 65536;
            s.inference.batch_size = 512;
            s.hardware.gpu_layers = -1;
        }
        Preset::Maximum => {
            s.inference.context_size = 131072;
            s.inference.batch_size = 1024;
            s.hardware.gpu_layers = -1;
        }
        Preset::Custom => {}
    }
}

#[cfg(test)]
mod runtime_mode_tests {
    use super::*;

    #[test]
    fn a_save_from_before_modes_keeps_its_meaning() {
        let mut manual = AppSettings::default();
        manual.runtime_auto = false;
        manual.runtime.mode = String::new();
        let manual = manual.normalized();
        assert_eq!(manual.runtime.mode, "manual");
        assert!(!manual.runtime_auto);

        let mut automatic = AppSettings::default();
        automatic.runtime.mode = String::new();
        assert_eq!(automatic.normalized().runtime.mode, "auto");

        let mut balanced = AppSettings::default();
        balanced.runtime.mode = "balanced".into();
        let balanced = balanced.normalized();
        assert!(balanced.runtime_auto, "a profile builds on automatic settings");
        assert_eq!(balanced.runtime.profile_name(), Some("balanced"));
        assert_eq!(AppSettings::default().runtime.profile_name(), None);
    }

    #[test]
    fn flipping_only_the_old_switch_changes_the_mode() {
        let previous = AppSettings::default().normalized();
        let mut from_old_client = previous.clone();
        from_old_client.runtime_auto = false;
        let saved = from_old_client.reconciled_with(&previous);
        assert_eq!(saved.runtime.mode, "manual");
        assert!(!saved.runtime_auto);

        // A client that sets the mode wins even when its switch is stale.
        let mut from_new_client = saved.clone();
        from_new_client.runtime.mode = "light".into();
        let saved = from_new_client.reconciled_with(&saved);
        assert_eq!(saved.runtime.mode, "light");
        assert!(saved.runtime_auto);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_changes_context() {
        let mut s = AppSettings::default();
        apply_preset(&mut s, Preset::Fast);
        assert_eq!(s.inference.context_size, 8192);
    }
}
