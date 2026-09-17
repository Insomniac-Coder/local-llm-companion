//! Per-model performance calibration: measure a small set of runtime
//! configurations on this machine and turn the results into profiles a person
//! can choose between (Fastest, Balanced, Light).
//!
//! Why measure instead of rule: on one hybrid laptop all 24 cores were fastest
//! and pinning to the performance cores lost 39% of generation speed, the
//! opposite of the usual advice; and going from 16 to 24 threads bought 1-5%
//! generation speed for 27 more points of CPU use. The right trade-off depends
//! on the processor, the model and whether layers sit on the GPU, so it is read
//! from a benchmark of this model on this machine, never assumed.
//!
//! Generation and prompt processing are measured separately because the
//! runtime runs them with separate thread counts (`--threads` and
//! `--threads-batch`): a profile can give short prompt bursts every core while
//! the long generation phase leaves the rest of the machine room.
//!
//! When the model uses the GPU, the micro-batch is measured too (512, 1,024
//! and 2,048, each with the placement the runtime fits at that size) and
//! chosen by the owner's speed rule (`speed_rule`); a load applies it while the
//! calibration still describes the machine.
//!
//! Everything here except `run_benchmark` and `fitted_placement` is pure and
//! unit-tested.

use serde::{Deserialize, Serialize};

/// Generation must stay within this share of the best measured speed for a
/// profile to use fewer threads. Starting points, visible in the profile
/// descriptions; the cards show real numbers so the trade-off is never hidden.
pub const BALANCED_SHARE: f64 = 0.95;
/// 75%, not 80%: on the audit's 4B sweep 12 threads measured 78.6% of the
/// fastest, so an 80% line made Light pick 18 threads, the same as Balanced.
pub const LIGHT_SHARE: f64 = 0.75;
/// Turning off spin-waiting (`--poll 0`) is kept only if it costs less than this.
pub const POLL_OFF_SHARE: f64 = 0.97;
/// A new calibration slower than the last comparable one by more than this is
/// reported as a regression.
pub const REGRESSION_SHARE: f64 = 0.90;

/// Where the model's layers run for this calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Every layer on the GPU: CPU threads only schedule work.
    Gpu,
    /// Some layers on the CPU: threads decide their speed.
    Hybrid,
    /// No GPU in use.
    Cpu,
}

/// The benchmark plan: thread counts to measure and the fixed settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub placement: Placement,
    /// Thread counts, largest first.
    pub threads: Vec<u32>,
    /// `-ngl` value: -1 for every layer, 0 for none.
    pub gpu_layers: i32,
    pub prompt_tokens: u32,
    pub generated_tokens: u32,
    pub repetitions: u32,
    /// `-ot` rules from the runtime's fit, one per entry: expert tensors of a
    /// mixture-of-experts model kept in RAM. Without them the benchmark would
    /// load every expert onto the GPU, a placement no load uses.
    #[serde(default)]
    pub tensor_overrides: Vec<String>,
    /// `-ub`; None leaves llama-bench's default (512).
    #[serde(default)]
    pub micro_batch: Option<u32>,
    /// `-lm none`: measured without mmap, as a load of this placement runs
    /// when RAM has room (`runtime_fit::load_without_mmap`). Loading without
    /// mmap changed prompt reading by ~48% on a mixture-of-experts model, so
    /// a calibration measured with mmap would describe a load that no longer
    /// happens.
    #[serde(default)]
    pub load_without_mmap: bool,
}

/// Thread counts worth measuring on a processor with `physical` cores, of which
/// `performance` are of the fastest class. Fractions of the physical count
/// cover every shape (uniform or hybrid, with or without SMT) without naming
/// one; the performance-core count is added on hybrid processors because it is
/// the conventional choice and the measurement should confirm or refute it.
pub fn thread_candidates(physical: usize, performance: usize, placement: Placement) -> Vec<u32> {
    let physical = physical.max(1);
    let mut counts: Vec<usize> = match placement {
        // With every layer on the GPU the CPU only dispatches work, so two
        // points show whether fewer threads lose anything.
        Placement::Gpu => vec![physical, physical.div_ceil(2), physical.div_ceil(4)],
        Placement::Hybrid | Placement::Cpu => vec![
            physical,
            (physical * 3).div_ceil(4),
            physical.div_ceil(2),
            physical.div_ceil(3),
            physical.div_ceil(4),
        ],
    };
    if performance > 0 && performance < physical {
        counts.push(performance);
    }
    counts.retain(|count| *count >= 1);
    counts.sort_unstable_by(|a, b| b.cmp(a));
    counts.dedup();
    counts.into_iter().map(|count| count as u32).collect()
}

pub fn plan(physical: usize, performance: usize, placement: Placement, gpu_layers: i32) -> Plan {
    Plan {
        placement,
        threads: thread_candidates(physical, performance, placement),
        gpu_layers: match placement {
            Placement::Cpu => 0,
            _ => gpu_layers,
        },
        // Short enough to finish in a minute or two on a CPU, long enough that
        // the per-token rate is stable.
        prompt_tokens: 256,
        generated_tokens: 64,
        repetitions: 4,
        tensor_overrides: Vec::new(),
        micro_batch: None,
        load_without_mmap: false,
    }
}

/// Micro-batch sizes a calibration compares when the model uses the GPU.
pub const MICRO_BATCH_CANDIDATES: [u32; 3] = [512, 1024, 2048];
/// Prompt length for the micro-batch runs. The thread sweep's 256 tokens fit
/// in one micro-batch of every size, which would make the sizes identical;
/// 4,096 is the prompt the owner's rule was approved on, and every candidate
/// then reads it in at least two steps.
pub const MICRO_BATCH_PROMPT_TOKENS: u32 = 4096;

/// The micro-batch sizes worth measuring at `context` tokens: those no larger
/// than the prompt the runs can use there. Empty when fewer than two remain,
/// since there is then nothing to choose between.
pub fn micro_batch_candidates(context: u32) -> Vec<u32> {
    let prompt = MICRO_BATCH_PROMPT_TOKENS.min(context);
    let sizes: Vec<u32> = MICRO_BATCH_CANDIDATES.iter().copied().filter(|size| *size <= prompt).collect();
    if sizes.len() < 2 {
        Vec::new()
    } else {
        sizes
    }
}

/// Pause between the two alternating micro-batch passes: back-to-back runs
/// can make the GPU throttle (owner instruction, 2026-09-16).
pub const MICRO_BATCH_PASS_PAUSE: std::time::Duration = std::time::Duration::from_secs(120);

/// The benchmark plan for one micro-batch: the thread sweep's plan with the
/// placement fitted at that micro-batch, the longer prompt, and the load mode
/// a load of that placement would use.
pub fn micro_batch_plan(base: &Plan, micro_batch: u32, fitted: &FittedPlacement, context: u32, load_without_mmap: bool) -> Plan {
    Plan {
        placement: fitted.placement,
        threads: base.threads.clone(),
        gpu_layers: fitted.gpu_layers,
        prompt_tokens: MICRO_BATCH_PROMPT_TOKENS.min(context.max(1)),
        generated_tokens: base.generated_tokens,
        repetitions: base.repetitions,
        tensor_overrides: fitted.tensor_overrides.clone(),
        micro_batch: Some(micro_batch),
        load_without_mmap,
    }
}

/// The order of the micro-batch runs: every size, then every size again in
/// reverse, so warming up over the runs cannot favour one size (the audit
/// saw the same setting differ by up to ~7% between runs).
pub fn micro_batch_run_order(sizes: &[u32]) -> Vec<(u32, u32)> {
    sizes
        .iter()
        .map(|size| (0, *size))
        .chain(sizes.iter().rev().map(|size| (1, *size)))
        .collect()
}

/// One measured micro-batch, with the placement the runtime fitted at that
/// size (a larger micro-batch needs a larger compute buffer, which can move
/// more of the model off the GPU).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicroBatchMeasurement {
    pub micro_batch: u32,
    /// `-ngl` of the placement fitted at this micro-batch.
    pub gpu_layers: i32,
    /// Blocks whose expert tensors that placement keeps in RAM: 0 for none,
    /// `u32::MAX` when one rule covers every block.
    #[serde(default)]
    pub expert_blocks_on_cpu: u32,
    pub threads: u32,
    pub prompt_tokens: u32,
    pub prompt: Option<Rate>,
    pub generation: Option<Rate>,
    /// 0 for the first pass, 1 for the reversed second pass.
    #[serde(default)]
    pub pass: u32,
    /// Measured without mmap (`Plan::load_without_mmap`).
    #[serde(default)]
    pub load_without_mmap: bool,
}

/// Each size's speeds, as the mean of its passes' medians, in the order the
/// sizes first appear. A pass without both speeds is left out.
fn micro_batch_speeds(measured: &[MicroBatchMeasurement]) -> Vec<(u32, crate::speed_rule::Speed)> {
    let mut sizes: Vec<u32> = Vec::new();
    for m in measured {
        if !sizes.contains(&m.micro_batch) {
            sizes.push(m.micro_batch);
        }
    }
    sizes
        .into_iter()
        .filter_map(|size| {
            let pairs: Vec<(f64, f64)> = measured
                .iter()
                .filter(|m| m.micro_batch == size)
                .filter_map(|m| Some((m.generation?.median, m.prompt?.median)))
                .collect();
            if pairs.is_empty() {
                return None;
            }
            let count = pairs.len() as f64;
            Some((
                size,
                crate::speed_rule::Speed {
                    generation_tps: pairs.iter().map(|(generation, _)| generation).sum::<f64>() / count,
                    prompt_tps: pairs.iter().map(|(_, prompt)| prompt).sum::<f64>() / count,
                },
            ))
        })
        .collect()
}

/// The micro-batch the owner's speed rule picks from the measurements, or
/// None unless 512 (the runtime's default, the baseline every other size is
/// judged against) and at least one other size were measured with both
/// speeds.
pub fn choose_micro_batch(measured: &[MicroBatchMeasurement]) -> Option<u32> {
    let speeds = micro_batch_speeds(measured);
    let has_default = speeds.iter().any(|(size, _)| *size == crate::runtime_fit::DEFAULT_MICRO_BATCH);
    if !has_default || speeds.len() < 2 {
        return None;
    }
    let options: Vec<crate::speed_rule::Speed> = speeds.iter().map(|(_, speed)| *speed).collect();
    crate::speed_rule::balanced_choice(&options).map(|index| speeds[index].0)
}

/// How many more expert blocks the calibration measured in RAM at `chosen`
/// than at 512: the change a load may repeat with the calibrated size
/// (`runtime_fit::calibrated_micro_batch_fits`). 0 when either size is missing
/// or the chosen size keeps no more; `u32::MAX` when one rule covers every
/// block at the chosen size but not at 512.
pub fn measured_extra_blocks(measured: &[MicroBatchMeasurement], chosen: u32) -> u32 {
    let blocks = |size: u32| measured.iter().find(|m| m.micro_batch == size).map(|m| m.expert_blocks_on_cpu);
    match (blocks(crate::runtime_fit::DEFAULT_MICRO_BATCH), blocks(chosen)) {
        (Some(at_default), Some(at_chosen)) => at_chosen.saturating_sub(at_default),
        _ => 0,
    }
}

/// Median and spread of repeated rate measurements (tokens per second).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub median: f64,
    pub p10: f64,
    pub p90: f64,
    pub samples: u32,
}

impl Rate {
    /// Summary of `samples`, dropping the first: the first repetition of a
    /// test ran measurably slower than the rest in the audit's benchmarks.
    pub fn from_samples(samples: &[f64]) -> Option<Self> {
        let kept: Vec<f64> = if samples.len() > 1 { samples[1..].to_vec() } else { samples.to_vec() };
        let mut sorted: Vec<f64> = kept.into_iter().filter(|value| value.is_finite()).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let at = |share: f64| {
            let position = (sorted.len() - 1) as f64 * share;
            let low = position.floor() as usize;
            let high = position.ceil() as usize;
            sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
        };
        Some(Self {
            median: round1(at(0.5)),
            p10: round1(at(0.1)),
            p90: round1(at(0.9)),
            samples: sorted.len() as u32,
        })
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// One measured configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub threads: u32,
    pub poll: u8,
    pub gpu_layers: i32,
    pub prompt: Option<Rate>,
    pub generation: Option<Rate>,
}

/// A configuration the person can choose, with what it was measured to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// fastest | balanced | light
    pub name: String,
    /// Generation threads (`--threads`).
    pub threads: u32,
    /// Prompt-processing threads (`--threads-batch`).
    pub threads_batch: u32,
    /// `--poll`: 50 spin-waits between operations, 0 sleeps.
    pub poll: u8,
    /// `--prio`: -1 lets other applications take the CPU first.
    pub priority: i8,
    pub gpu_layers: i32,
    pub generation_tps: f64,
    pub prompt_tps: f64,
    /// Share of the fastest measured generation speed, 0-100.
    pub generation_share: u32,
    /// Cores left free while generating, of the physical cores.
    pub free_cores_generating: u32,
    pub description: String,
}

/// Pick the three profiles from the measurements. None when nothing usable
/// was measured.
pub fn choose_profiles(measurements: &[Measurement], physical: usize) -> Option<Vec<Profile>> {
    let physical = physical.max(1) as u32;
    let spinning: Vec<&Measurement> = measurements.iter().filter(|m| m.poll != 0).collect();
    let gen = |m: &Measurement| m.generation.map(|rate| rate.median).unwrap_or(0.0);
    let prompt = |m: &Measurement| m.prompt.map(|rate| rate.median).unwrap_or(0.0);

    let fastest = spinning
        .iter()
        .copied()
        .filter(|m| gen(m) > 0.0)
        .max_by(|a, b| gen(a).total_cmp(&gen(b)).then(prompt(a).total_cmp(&prompt(b))))?;
    let best_gen = gen(fastest);
    let best_prompt = spinning
        .iter()
        .copied()
        .filter(|m| prompt(m) > 0.0)
        .max_by(|a, b| prompt(a).total_cmp(&prompt(b)))
        .unwrap_or(fastest);

    // The fewest generation threads that keep at least `share` of the best.
    let fewest_within = |share: f64| -> &Measurement {
        spinning
            .iter()
            .copied()
            .filter(|m| gen(m) >= best_gen * share)
            .min_by_key(|m| m.threads)
            .unwrap_or(fastest)
    };
    let balanced = fewest_within(BALANCED_SHARE);
    let light = fewest_within(LIGHT_SHARE);

    // Light also stops spin-waiting when that measured nearly free.
    let light_poll = measurements
        .iter()
        .find(|m| m.poll == 0 && m.threads == light.threads)
        .filter(|quiet| gen(quiet) >= gen(light) * POLL_OFF_SHARE)
        .map(|_| 0)
        .unwrap_or(50);
    let light_gen = measurements
        .iter()
        .find(|m| m.poll == light_poll && m.threads == light.threads)
        .map(gen)
        .unwrap_or(gen(light));

    let share = |value: f64| ((value / best_gen) * 100.0).round().clamp(0.0, 100.0) as u32;
    let free = |threads: u32| physical.saturating_sub(threads);
    Some(vec![
        Profile {
            name: "fastest".into(),
            threads: fastest.threads,
            threads_batch: best_prompt.threads,
            poll: 50,
            priority: 0,
            gpu_layers: fastest.gpu_layers,
            generation_tps: gen(fastest),
            prompt_tps: prompt(best_prompt),
            generation_share: 100,
            free_cores_generating: free(fastest.threads),
            description: "The highest measured generation speed.".into(),
        },
        Profile {
            name: "balanced".into(),
            threads: balanced.threads,
            threads_batch: best_prompt.threads,
            poll: 50,
            priority: 0,
            gpu_layers: balanced.gpu_layers,
            generation_tps: gen(balanced),
            prompt_tps: prompt(best_prompt),
            generation_share: share(gen(balanced)),
            free_cores_generating: free(balanced.threads),
            description: format!(
                "The fewest generation threads within {}% of the fastest; prompts still use the fastest prompt setting.",
                (100.0 - BALANCED_SHARE * 100.0).round()
            ),
        },
        Profile {
            name: "light".into(),
            threads: light.threads,
            threads_batch: light.threads,
            poll: light_poll,
            priority: -1,
            gpu_layers: light.gpu_layers,
            generation_tps: light_gen,
            prompt_tps: prompt(light),
            generation_share: share(light_gen),
            free_cores_generating: free(light.threads),
            description: format!(
                "Keeps at least {}% of the fastest generation speed with the most room for other applications, and yields the CPU to them.",
                (LIGHT_SHARE * 100.0).round()
            ),
        },
    ])
}

/// What changed since the last comparable calibration, when it got slower.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Regression {
    pub previous_tps: f64,
    pub current_tps: f64,
    /// Negative: percentage slower.
    pub change_percent: f64,
    pub previous_at: String,
}

pub fn regression(previous: &Calibration, current_fastest_tps: f64) -> Option<Regression> {
    let before = previous.profiles.iter().find(|p| p.name == "fastest")?.generation_tps;
    if before <= 0.0 || current_fastest_tps >= before * REGRESSION_SHARE {
        return None;
    }
    Some(Regression {
        previous_tps: before,
        current_tps: current_fastest_tps,
        change_percent: round1((current_fastest_tps / before - 1.0) * 100.0),
        previous_at: previous.created_at.clone(),
    })
}

/// Conditions a measurement depends on. Two calibrations are only compared
/// when these match; a result on battery power or another runtime build is a
/// different experiment, not a regression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Environment {
    pub device_fingerprint: String,
    pub runtime_build: String,
    pub power_source: String,
    pub gpu_driver: String,
    pub cpu_topology: String,
}

impl Environment {
    pub fn comparable(&self, other: &Environment) -> bool {
        self.device_fingerprint == other.device_fingerprint
            && self.runtime_build == other.runtime_build
            && self.power_source == other.power_source
    }
}

/// A stored calibration of one model on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub id: String,
    pub model_id: String,
    /// Model file identity: file name and size, so a replaced file is not
    /// mistaken for the one that was measured.
    pub model_key: String,
    pub created_at: String,
    pub environment: Environment,
    pub plan: Plan,
    pub measurements: Vec<Measurement>,
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub regression: Option<Regression>,
    /// Micro-batch sizes measured with their own fitted placement. Empty for
    /// CPU-only calibrations and for calibrations saved before these were
    /// measured.
    #[serde(default)]
    pub micro_batches: Vec<MicroBatchMeasurement>,
    /// The micro-batch the owner's speed rule chose from `micro_batches`; a
    /// load uses it instead of the default rule while this calibration still
    /// describes the machine.
    #[serde(default)]
    pub micro_batch: Option<u32>,
}

pub fn model_key(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    format!("{name}:{size}")
}

/// Parse `llama-bench -o json` output into measurements. Each entry is either
/// a prompt test (n_gen 0) or a generation test (n_prompt 0).
pub fn parse_bench_json(json: &str) -> Vec<Measurement> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    let mut out: Vec<Measurement> = Vec::new();
    for entry in entries {
        let threads = entry["n_threads"].as_u64().unwrap_or(0) as u32;
        let poll = entry["poll"].as_u64().unwrap_or(50) as u8;
        let gpu_layers = entry["n_gpu_layers"].as_i64().unwrap_or(0) as i32;
        let samples: Vec<f64> = entry["samples_ts"]
            .as_array()
            .map(|values| values.iter().filter_map(|value| value.as_f64()).collect())
            .unwrap_or_default();
        let Some(rate) = Rate::from_samples(&samples) else {
            continue;
        };
        let is_prompt = entry["n_prompt"].as_u64().unwrap_or(0) > 0 && entry["n_gen"].as_u64().unwrap_or(0) == 0;
        let index = match out
            .iter()
            .position(|m| m.threads == threads && m.poll == poll && m.gpu_layers == gpu_layers)
        {
            Some(index) => index,
            None => {
                out.push(Measurement { threads, poll, gpu_layers, prompt: None, generation: None });
                out.len() - 1
            }
        };
        if is_prompt {
            out[index].prompt = Some(rate);
        } else {
            out[index].generation = Some(rate);
        }
    }
    out
}

/// Arguments for one llama-bench invocation over the plan's thread counts.
/// The fit's `-ot` rules are joined with `;`: llama-bench reads a comma as the
/// start of another test.
pub fn bench_args(model: &std::path::Path, plan: &Plan, threads: &[u32], poll: u8) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        model.display().to_string(),
        "-p".into(),
        plan.prompt_tokens.to_string(),
        "-n".into(),
        plan.generated_tokens.to_string(),
        "-r".into(),
        plan.repetitions.to_string(),
        "-o".into(),
        "json".into(),
        "-t".into(),
        threads.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
        "--poll".into(),
        poll.to_string(),
        "-ngl".into(),
        plan.gpu_layers.to_string(),
    ];
    if let Some(micro_batch) = plan.micro_batch {
        args.extend(crate::runtime_fit::micro_batch_args(micro_batch));
    }
    if plan.placement == Placement::Cpu {
        args.extend(["-dev".into(), "none".into()]);
    } else {
        if !plan.tensor_overrides.is_empty() {
            args.extend(["-ot".into(), plan.tensor_overrides.join(";")]);
        }
        if plan.load_without_mmap {
            args.extend(["-lm".into(), "none".into()]);
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Running the benchmark (not unit-tested: it needs a model and the runtime)
// ---------------------------------------------------------------------------

/// A runtime tool next to llama-server (`llama-bench`, `llama-fit-params`).
pub fn runtime_tool(server: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let path = server.parent()?.join(file);
    path.is_file().then_some(path)
}

fn hidden(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    #[cfg(not(windows))]
    let _ = command;
}

/// Run a runtime tool and return (success, stdout, stderr).
async fn run_tool(
    tool: &std::path::Path,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<(bool, String, String), String> {
    let mut command = tokio::process::Command::new(tool);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = tool.parent() {
        command.current_dir(dir);
    }
    hidden(&mut command);
    let child = command.spawn().map_err(|error| format!("could not start {}: {error}", tool.display()))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
        Ok(Err(error)) => Err(format!("{} failed: {error}", tool.display())),
        Err(_) => Err(format!("{} did not finish within {} s", tool.display(), timeout.as_secs())),
    }
}

/// Where the runtime's fit put the model, in the form llama-bench takes.
#[derive(Debug, Clone, PartialEq)]
pub struct FittedPlacement {
    pub placement: Placement,
    /// `-ngl`: -1 for every layer, 0 for none.
    pub gpu_layers: i32,
    /// `-ot` rules keeping expert tensors in RAM, one per entry.
    pub tensor_overrides: Vec<String>,
    /// The same fit as the load path reads it.
    pub fit: crate::runtime_fit::Fit,
}

impl FittedPlacement {
    /// No GPU in use.
    pub fn cpu() -> Self {
        Self {
            placement: Placement::Cpu,
            gpu_layers: 0,
            tensor_overrides: Vec::new(),
            fit: crate::runtime_fit::Fit::Layers(0),
        }
    }

    /// Blocks whose expert tensors stay in RAM (0 when none).
    pub fn expert_blocks_on_cpu(&self) -> u32 {
        match self.fit {
            crate::runtime_fit::Fit::ExpertsOnCpu { blocks, .. } => blocks,
            _ => 0,
        }
    }
}

/// Read `llama-fit-params` output into a placement. Expert rules make a
/// placement hybrid even with every layer on the GPU: the CPU then runs the
/// experts each token picks, so threads decide part of the speed.
pub fn read_fitted_placement(stdout: &str) -> Result<FittedPlacement, String> {
    let fit = crate::runtime_fit::parse_fit_output(stdout)?;
    let gpu_layers = crate::runtime_fit::fitted_gpu_layers(stdout).ok_or_else(|| format!("unexpected fit output: {}", stdout.trim()))?;
    let tensor_overrides = crate::runtime_fit::tensor_overrides(stdout);
    let placement = if gpu_layers == 0 {
        Placement::Cpu
    } else if gpu_layers < 0 && tensor_overrides.is_empty() {
        Placement::Gpu
    } else {
        Placement::Hybrid
    };
    Ok(FittedPlacement { placement, gpu_layers, tensor_overrides, fit })
}

/// Where the runtime's own fit puts this model at `context` tokens and the
/// given micro-batch: every layer on the GPU, some, or none, and which expert
/// tensors stay in RAM. The runtime is the authority here; the app's
/// header-based estimate overstated the cache of hybrid-attention models
/// several-fold and called a model that fits entirely "hybrid".
pub async fn fitted_placement(
    fit_tool: &std::path::Path,
    model: &std::path::Path,
    context: u32,
    micro_batch: u32,
) -> Result<FittedPlacement, String> {
    let mut args = vec!["-m".to_string(), model.display().to_string(), "-c".into(), context.to_string()];
    args.extend(crate::runtime_fit::micro_batch_args(micro_batch));
    let (ok, stdout, stderr) = run_tool(fit_tool, &args, std::time::Duration::from_secs(300)).await?;
    if !ok {
        let reason = stderr.lines().find(|line| line.contains(" E ")).unwrap_or("the runtime could not read the model");
        return Err(reason.trim().to_string());
    }
    read_fitted_placement(&stdout)
}

/// Run llama-bench over the given thread counts and parse the result.
pub async fn run_benchmark(
    bench_tool: &std::path::Path,
    model: &std::path::Path,
    plan: &Plan,
    threads: &[u32],
    poll: u8,
) -> Result<Vec<Measurement>, String> {
    let args = bench_args(model, plan, threads, poll);
    // Generous: a CPU-only 27B model at the smallest thread count is slow.
    let (ok, stdout, stderr) = run_tool(bench_tool, &args, std::time::Duration::from_secs(1800)).await?;
    if !ok {
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
        return Err(format!("the benchmark failed: {tail}"));
    }
    let parsed = parse_bench_json(&stdout);
    if parsed.is_empty() {
        return Err("the benchmark produced no measurements".into());
    }
    Ok(parsed)
}

/// `llama-server --version`, the runtime build a calibration belongs to.
pub async fn runtime_build(server: &std::path::Path) -> String {
    match run_tool(server, &["--version".to_string()], std::time::Duration::from_secs(20)).await {
        Ok((_, stdout, stderr)) => format!("{stdout}\n{stderr}")
            .lines()
            .find(|line| line.trim_start().starts_with("version:"))
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| "unknown".into()),
        Err(_) => "unknown".into(),
    }
}

/// "ac", "battery" or "unknown". A laptop on battery runs a different power
/// plan, so its measurements are not compared with ones taken on AC.
pub fn power_source() -> String {
    platform_power::source()
}

#[cfg(windows)]
mod platform_power {
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    pub fn source() -> String {
        let mut status = SystemPowerStatus::default();
        // SAFETY: the struct matches SYSTEM_POWER_STATUS and outlives the call.
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return "unknown".into();
        }
        match status.ac_line_status {
            1 => "ac",
            0 => "battery",
            _ => "unknown",
        }
        .into()
    }
}

#[cfg(target_os = "linux")]
mod platform_power {
    pub fn source() -> String {
        let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
            return "unknown".into();
        };
        for entry in entries.flatten() {
            let kind = std::fs::read_to_string(entry.path().join("type")).unwrap_or_default();
            if kind.trim() == "Mains" {
                let online = std::fs::read_to_string(entry.path().join("online")).unwrap_or_default();
                return if online.trim() == "1" { "ac" } else { "battery" }.into();
            }
        }
        "unknown".into()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform_power {
    pub fn source() -> String {
        "unknown".into()
    }
}

/// The NVIDIA driver version when nvidia-smi is available.
pub async fn gpu_driver() -> String {
    let mut command = tokio::process::Command::new("nvidia-smi");
    command
        .args(["--query-gpu=driver_version", "--format=csv,noheader"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    hidden(&mut command);
    match command.spawn() {
        Ok(child) => match tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output()).await {
            Ok(Ok(output)) => String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or("").trim().to_string(),
            _ => String::new(),
        },
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(median: f64) -> Option<Rate> {
        Some(Rate { median, p10: median, p90: median, samples: 3 })
    }

    fn m(threads: u32, poll: u8, prompt: f64, generation: f64) -> Measurement {
        Measurement { threads, poll, gpu_layers: 0, prompt: rate(prompt), generation: rate(generation) }
    }

    #[test]
    fn candidates_follow_the_topology_not_a_processor_model() {
        // 24 physical cores, 8 of the fastest class (a hybrid laptop).
        assert_eq!(thread_candidates(24, 8, Placement::Cpu), vec![24, 18, 12, 8, 6]);
        // A uniform 8-core processor: no separate performance count.
        assert_eq!(thread_candidates(8, 8, Placement::Hybrid), vec![8, 6, 4, 3, 2]);
        // All layers on the GPU: a few points are enough.
        assert_eq!(thread_candidates(16, 16, Placement::Gpu), vec![16, 8, 4]);
        assert_eq!(thread_candidates(1, 1, Placement::Cpu), vec![1]);
    }

    #[test]
    fn profiles_trade_generation_speed_for_room_as_measured() {
        // Shaped like the audit's 4B CPU sweep: 16 threads within 1% of 24.
        let measurements = vec![
            m(24, 50, 283.0, 25.2),
            m(18, 50, 255.0, 25.0),
            m(12, 50, 190.0, 19.8),
            m(8, 50, 157.0, 19.3),
            m(6, 50, 130.0, 16.0),
            m(8, 0, 150.0, 19.1),
        ];
        let profiles = choose_profiles(&measurements, 24).unwrap();
        let by = |name: &str| profiles.iter().find(|p| p.name == name).unwrap().clone();
        let fastest = by("fastest");
        assert_eq!((fastest.threads, fastest.threads_batch), (24, 24));
        let balanced = by("balanced");
        assert_eq!(balanced.threads, 18, "fewest threads within 5% of 25.2");
        assert_eq!(balanced.threads_batch, 24, "prompts keep the fastest prompt setting");
        assert_eq!(balanced.free_cores_generating, 6);
        let light = by("light");
        // 75% of 25.2 is 18.9: 8 threads (19.3) qualify, 6 threads (16.0) do not.
        assert_eq!(light.threads, 8);
        assert_ne!(light.threads, balanced.threads, "profiles must not collapse into one");
        assert_eq!(light.poll, 0, "poll off cost 1%, under the 3% allowance");
        assert_eq!(light.priority, -1);
        assert!(light.generation_share >= 75);
    }

    #[test]
    fn nothing_measured_gives_no_profiles() {
        assert!(choose_profiles(&[], 8).is_none());
    }

    #[test]
    fn a_slower_comparable_calibration_is_a_regression() {
        let previous = Calibration {
            id: "a".into(),
            model_id: "m".into(),
            model_key: "m.gguf:1".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            environment: Environment::default(),
            plan: plan(8, 8, Placement::Cpu, 0),
            measurements: vec![],
            profiles: choose_profiles(&[m(8, 50, 100.0, 42.1)], 8).unwrap(),
            regression: None,
            micro_batches: vec![],
            micro_batch: None,
        };
        let found = regression(&previous, 34.8).unwrap();
        assert_eq!(found.change_percent, -17.3);
        assert!(regression(&previous, 40.0).is_none(), "5% is within noise");
    }

    #[test]
    fn llama_bench_json_becomes_paired_measurements() {
        let json = r#"[
            {"n_threads": 8, "poll": 50, "n_gpu_layers": 0, "n_prompt": 256, "n_gen": 0, "samples_ts": [90.0, 100.0, 102.0, 98.0]},
            {"n_threads": 8, "poll": 50, "n_gpu_layers": 0, "n_prompt": 0, "n_gen": 64, "samples_ts": [18.0, 20.0, 21.0, 20.5]}
        ]"#;
        let parsed = parse_bench_json(json);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].prompt.unwrap().median, 100.0);
        assert_eq!(parsed[0].generation.unwrap().median, 20.5);
        assert_eq!(parsed[0].generation.unwrap().samples, 3);
    }

    const MOE_FIT: &str = r#"-c 32768 -ngl 49 -ot "blk\.24\.ffn_(gate|gate_up|down).*=CPU,blk\.25\.ffn_(up|down|gate_up|gate)_(ch|)exps=CPU""#;

    #[test]
    fn the_fit_is_read_into_a_placement_llama_bench_can_repeat() {
        let moe = read_fitted_placement(MOE_FIT).unwrap();
        assert_eq!(moe.placement, Placement::Hybrid, "the CPU runs the experts kept in RAM");
        assert_eq!(moe.gpu_layers, 49);
        assert_eq!(moe.tensor_overrides.len(), 2);
        assert_eq!(moe.expert_blocks_on_cpu(), 2);
        let all = read_fitted_placement("-c 8192 -ngl -1").unwrap();
        assert_eq!((all.placement, all.gpu_layers, all.expert_blocks_on_cpu()), (Placement::Gpu, -1, 0));
        assert!(all.tensor_overrides.is_empty());
        assert_eq!(read_fitted_placement("-c 16384 -ngl 41").unwrap().placement, Placement::Hybrid);
        assert_eq!(read_fitted_placement("-c 16384 -ngl 0").unwrap().placement, Placement::Cpu);
        assert!(read_fitted_placement("garbage").is_err());
    }

    #[test]
    fn bench_args_repeat_the_fitted_placement_and_micro_batch() {
        let fitted = read_fitted_placement(MOE_FIT).unwrap();
        let base = Plan { tensor_overrides: fitted.tensor_overrides.clone(), ..plan(24, 8, Placement::Hybrid, fitted.gpu_layers) };
        let has = |args: &[String], key: &str, value: &str| args.windows(2).any(|pair| pair[0] == key && pair[1] == value);
        let sweep = bench_args(std::path::Path::new("model.gguf"), &base, &[24, 18], 50);
        assert!(has(&sweep[..], "-ot", r"blk\.24\.ffn_(gate|gate_up|down).*=CPU;blk\.25\.ffn_(up|down|gate_up|gate)_(ch|)exps=CPU"), "{sweep:?}");
        assert!(has(&sweep[..], "-ngl", "49") && has(&sweep[..], "-t", "24,18") && has(&sweep[..], "-p", "256"));
        assert!(!sweep.contains(&"-ub".to_string()), "the thread sweep keeps llama-bench's micro-batch");

        let at_1024 = micro_batch_plan(&base, 1024, &fitted, 32_768, false);
        let args = bench_args(std::path::Path::new("model.gguf"), &at_1024, &[24], 50);
        assert!(has(&args[..], "-ub", "1024") && has(&args[..], "-p", "4096") && has(&args[..], "-n", "64") && has(&args[..], "-r", "4"));
        assert!(!args.contains(&"-b".to_string()), "llama-bench's logical batch (2,048) already holds 1,024");
        assert!(!args.contains(&"-lm".to_string()), "mapped, as a load without room for a copy runs");
        let pinned = bench_args(std::path::Path::new("model.gguf"), &micro_batch_plan(&base, 1024, &fitted, 32_768, true), &[24], 50);
        assert!(has(&pinned[..], "-lm", "none"), "measured the way the load runs: {pinned:?}");
        let small_context = micro_batch_plan(&base, 1024, &fitted, 2_048, false);
        assert_eq!(small_context.prompt_tokens, 2_048, "the prompt never exceeds the context");

        let cpu = plan(8, 8, Placement::Cpu, 0);
        let cpu_args = bench_args(std::path::Path::new("model.gguf"), &cpu, &[8], 50);
        assert!(has(&cpu_args[..], "-dev", "none") && !cpu_args.contains(&"-ot".to_string()));
    }

    #[test]
    fn micro_batch_candidates_fit_the_prompt_the_context_allows() {
        assert_eq!(micro_batch_candidates(32_768), vec![512, 1024, 2048]);
        assert_eq!(micro_batch_candidates(2_048), vec![512, 1024, 2048]);
        assert_eq!(micro_batch_candidates(1_500), vec![512, 1024]);
        assert!(micro_batch_candidates(1_000).is_empty(), "one size is not a choice");
    }

    #[test]
    fn the_speed_rule_picks_the_micro_batch_from_the_measurements() {
        let measured = |micro_batch: u32, blocks: u32, prompt: f64, generation: f64| MicroBatchMeasurement {
            micro_batch,
            gpu_layers: 49,
            expert_blocks_on_cpu: blocks,
            threads: 24,
            prompt_tokens: 4096,
            prompt: rate(prompt),
            generation: rate(generation),
            pass: 0,
            load_without_mmap: false,
        };
        // The 30B mixture-of-experts measurements, each size with its own fit.
        let table = vec![measured(512, 25, 1_617.0, 67.5), measured(1024, 26, 2_369.0, 66.1), measured(2048, 28, 3_091.0, 59.8)];
        assert_eq!(choose_micro_batch(&table), Some(1024));
        assert_eq!(measured_extra_blocks(&table, 1024), 1);
        assert_eq!(measured_extra_blocks(&table, 512), 0);
        let without_512 = vec![MicroBatchMeasurement { generation: None, ..table[0].clone() }, table[1].clone(), table[2].clone()];
        assert_eq!(choose_micro_batch(&without_512), None, "nothing to judge the other sizes against without 512");
        assert_eq!(choose_micro_batch(&table[..1]), None, "one size is not a choice");
        assert_eq!(choose_micro_batch(&[]), None);
        // Two passes: a noisy high reading in one pass is averaged with the
        // other. 2,048 alone at 64.5 would lose only 4.4% and pass; with its
        // second pass (55.1) it loses 11% and does not.
        let noisy = vec![
            measured(512, 25, 1_617.0, 67.5),
            measured(1024, 26, 2_369.0, 66.1),
            measured(2048, 28, 3_091.0, 64.5),
            MicroBatchMeasurement { pass: 1, ..measured(2048, 28, 3_091.0, 55.1) },
            MicroBatchMeasurement { pass: 1, ..measured(1024, 26, 2_369.0, 66.1) },
            MicroBatchMeasurement { pass: 1, ..measured(512, 25, 1_617.0, 67.5) },
        ];
        assert_eq!(choose_micro_batch(&noisy), Some(1024));
        assert_eq!(choose_micro_batch(&noisy[..3]), Some(2048), "a single pass would have been fooled");
    }

    #[test]
    fn micro_batch_runs_alternate_in_two_passes() {
        assert_eq!(micro_batch_run_order(&[512, 1024, 2048]), vec![(0, 512), (0, 1024), (0, 2048), (1, 2048), (1, 1024), (1, 512)]);
        assert!(micro_batch_run_order(&[]).is_empty());
    }

    #[test]
    fn a_calibration_saved_before_micro_batches_still_loads() {
        let current = Calibration {
            id: "a".into(),
            model_id: "m".into(),
            model_key: "m.gguf:1".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            environment: Environment::default(),
            plan: plan(8, 8, Placement::Gpu, -1),
            measurements: vec![m(8, 50, 100.0, 42.1)],
            profiles: vec![],
            regression: None,
            micro_batches: vec![],
            micro_batch: Some(1024),
        };
        let mut old = serde_json::to_value(&current).unwrap();
        let record = old.as_object_mut().unwrap();
        record.remove("micro_batches");
        record.remove("micro_batch");
        let plan = record.get_mut("plan").unwrap().as_object_mut().unwrap();
        plan.remove("tensor_overrides");
        plan.remove("micro_batch");
        plan.remove("load_without_mmap");
        let loaded: Calibration = serde_json::from_value(old).unwrap();
        assert!(loaded.micro_batches.is_empty());
        assert_eq!(loaded.micro_batch, None);
        assert!(loaded.plan.tensor_overrides.is_empty());
        assert_eq!(loaded.plan.micro_batch, None);
        assert!(!loaded.plan.load_without_mmap);
        let measurement: MicroBatchMeasurement = serde_json::from_value(serde_json::json!({
            "micro_batch": 1024, "gpu_layers": 49, "threads": 24, "prompt_tokens": 4096, "prompt": null, "generation": null
        }))
        .unwrap();
        assert_eq!((measurement.pass, measurement.load_without_mmap, measurement.expert_blocks_on_cpu), (0, false, 0));
    }
}
