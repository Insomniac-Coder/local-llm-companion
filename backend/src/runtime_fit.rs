//! Context sizing from the runtime's own memory fit.
//!
//! The load path used to size the context from a formula over the GGUF header
//! (every layer counted as full attention). For a hybrid-attention 27B model
//! that formula overstated the cache several-fold: it cut a 32K request to 16K
//! and labelled the model "hybrid, about a quarter of the weights on the CPU",
//! while llama.cpp's own fit placed all 65 layers on the GPU at short context
//! and 63 of 65 at 16K. The runtime knows every architecture it can load, so
//! it is asked instead: `llama-fit-params -m <model> -c <tokens>` prints the
//! layer count that fits, `-ngl -1` meaning every layer.
//!
//! Every layer that spills to the CPU is expensive (an 8B model at 88.7 tok/s
//! with all layers on the GPU dropped to 64.2 with two on the CPU), so the
//! search looks for the largest context that keeps every layer on the GPU.
//!
//! The micro-batch (`--ubatch-size`, tokens computed per step while reading a
//! prompt) and the load mode are chosen here too, by the owner's speed rule
//! (`speed_rule`): a larger micro-batch reads prompts faster but its compute
//! buffer takes VRAM the model could use, so the probe is repeated at the
//! larger size (`default_micro_batch`).

use serde::{Deserialize, Serialize};

/// llama-server's own micro-batch when none is given.
pub const DEFAULT_MICRO_BATCH: u32 = 512;
/// The larger micro-batch a load takes without calibration when the fit
/// allows it (`default_micro_batch`).
pub const BALANCED_MICRO_BATCH: u32 = 1024;
/// llama.cpp's logical batch when none is given. It caps the micro-batch, so
/// a larger micro-batch needs the logical batch raised with it.
pub const RUNTIME_DEFAULT_BATCH: u32 = 2048;
/// RAM left free beside the copy of the weights a load without mmap makes
/// (`load_without_mmap`).
pub const PINNED_COPY_HEADROOM_MIB: u64 = 4096;
/// Runtime notes about the micro-batch and the load mode start with these, so
/// a CPU fallback (which resets both) can drop them.
pub const MICRO_BATCH_NOTE_PREFIX: &str = "Micro-batch ";
pub const LOAD_MODE_NOTE_PREFIX: &str = "Loading without mmap";
pub const NGRAM_LENGTH_NOTE_PREFIX: &str = "N-gram drafts";

/// N-gram draft length for a placement that keeps weights in RAM while using
/// the GPU. Measured with full-rewrite and prose tasks (tok/s, higher is
/// faster), llama.cpp's default length being 48:
/// - 30B mixture-of-experts model with experts in RAM: rewrite 63.6 at 48,
///   90.6 at 24 (+42%), 88.4 at 12; prose 57.6 / 58.9 / 58.9. A rejected
///   long draft costs a verification batch whose experts partly run on the
///   CPU.
/// - Models fully on the GPU: the default was marginally faster (8B rewrite
///   167.0 at 48 vs 163.3 at 24; 12B 100.7 vs 99.0), so they keep it.
///
/// CPU-only loads keep the default until measured.
pub const SPLIT_NGRAM_DRAFT_LENGTH: u32 = 24;

/// The n-gram draft length a load should pass, or None for the runtime's
/// default: `SPLIT_NGRAM_DRAFT_LENGTH` when the fit keeps weights in RAM while
/// using the GPU and n-gram drafting is on.
pub fn ngram_draft_length(fit: Fit, speculative: &str) -> Option<u32> {
    let ngram = speculative.split(',').any(|kind| kind.trim() == "ngram-simple");
    (ngram && fit.uses_gpu() && fit.keeps_weights_in_ram()).then_some(SPLIT_NGRAM_DRAFT_LENGTH)
}

/// What one probe of the runtime reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    /// Every layer on the GPU.
    AllLayers,
    /// Only this many layers fit.
    Layers(i32),
    /// Expert tensors of `blocks` blocks kept in RAM (the fit tool adds
    /// `-ot ...exps=CPU` rules; `u32::MAX` when one rule covers every block),
    /// with `gpu_layers` as the tool printed them (`-ngl`, one more than the
    /// block count when every layer is on the GPU). When even the dense
    /// weights do not fit with every expert in RAM, the tool lowers `-ngl` and
    /// prints rules only for the blocks still on the GPU, so fewer rules can
    /// mean fewer layers on the GPU rather than fewer experts in RAM: two fits
    /// compare on both numbers. Only the experts each token picks run on the
    /// CPU. Measured on a 30B mixture-of-experts model: 66.0 tok/s with the
    /// fit's own choice (~25 blocks), 48.5 splitting whole layers instead, and
    /// fewer blocks on the CPU is faster (24: 65.6, 32: 50.3, 40: 44.4).
    ExpertsOnCpu { gpu_layers: i32, blocks: u32 },
}

/// `-ngl` as a rank: -1 (every layer) above any count.
fn gpu_layer_rank(layers: i32) -> i32 {
    if layers < 0 {
        i32::MAX
    } else {
        layers
    }
}

impl Fit {
    /// Some of the model runs on the GPU.
    pub fn uses_gpu(self) -> bool {
        match self {
            Fit::AllLayers => true,
            Fit::Layers(layers) => layers > 0,
            Fit::ExpertsOnCpu { gpu_layers, .. } => gpu_layers != 0,
        }
    }

    /// Weights stay in RAM while the GPU runs the rest: whole layers or the
    /// expert tensors of some blocks.
    pub fn keeps_weights_in_ram(self) -> bool {
        match self {
            Fit::AllLayers => false,
            Fit::Layers(layers) => layers > 0,
            Fit::ExpertsOnCpu { blocks, .. } => blocks > 0,
        }
    }
}

/// Largest context in `step`-token increments, between `floor` and
/// `requested`, at which the runtime reports every layer on the GPU; None when
/// even `floor` does not fit. `probe_batch` answers for up to `width` contexts
/// per round: first `requested` and `floor` (with points between them when the
/// width allows), then points spread between the largest step known to fit and
/// the smallest known not to, so each round cuts the range `width + 1` ways.
/// Width 1 is a binary search.
pub async fn largest_full_gpu_context<F, Fut>(requested: u32, floor: u32, step: u32, width: usize, mut probe_batch: F) -> Result<Option<u32>, String>
where
    F: FnMut(Vec<u32>) -> Fut,
    Fut: std::future::Future<Output = Vec<Result<Fit, String>>>,
{
    let step = step.max(1);
    let width = width.max(1);
    let floor = floor.min(requested).max(1);
    // Invariant once the floor fits: step `low` fits, step `high` does not.
    let mut low = floor / step;
    let mut high = requested.div_ceil(step);
    let mut ask = |contexts: Vec<u32>| {
        let asked = contexts.clone();
        let answer = probe_batch(contexts);
        async move {
            let results = answer.await;
            if results.len() != asked.len() {
                return Err("the fit probe gave no answer".to_string());
            }
            asked.into_iter().zip(results).map(|(context, result)| result.map(|fit| (context, fit))).collect::<Result<Vec<_>, String>>()
        }
    };
    let mut first = vec![requested];
    if width > 1 {
        first.push(floor);
        first.extend(spread(low, high, width - 2).into_iter().map(|unit| unit * step));
    }
    let mut answers = ask(first).await?;
    if answers[0].1 == Fit::AllLayers {
        return Ok(Some(requested));
    }
    if answers.len() == 1 {
        answers.extend(ask(vec![floor]).await?);
    }
    if answers[1].1 != Fit::AllLayers {
        return Ok(None);
    }
    let narrow = |answers: &[(u32, Fit)], low: &mut u32, high: &mut u32| {
        for (context, fit) in answers {
            let unit = context / step;
            if *fit == Fit::AllLayers && context % step == 0 && unit > *low && unit < *high {
                *low = unit;
            }
        }
        for (context, fit) in answers {
            let unit = context / step;
            if *fit != Fit::AllLayers && context % step == 0 && unit > *low && unit < *high {
                *high = unit;
            }
        }
    };
    narrow(&answers[2..], &mut low, &mut high);
    while high - low > 1 {
        let points: Vec<u32> = spread(low, high, width).into_iter().map(|unit| unit * step).collect();
        let answers = ask(points).await?;
        narrow(&answers, &mut low, &mut high);
    }
    Ok(Some((low * step).max(floor)))
}

/// Cache precision and fit margin (MiB of VRAM left free) combinations, in the
/// order a load prefers them when they give the same speed: most headroom
/// first, then the unquantised cache. Measured: once every layer is on the GPU
/// the speed is the same for all of them (8B at 16K: f16 55.4, q8_0 57.8
/// tok/s); while layers spill, more layers on the GPU is faster at both
/// generation and prompt reading (14B at 16K: 13.1 tok/s and 790 prompt at 41
/// layers, 32.7 and 965 with every layer and a q8_0 cache; 27B at 32K: 18.8 →
/// 36.2 generation, 734 → 877 prompt). The owner's balance rule
/// (`speed_rule`, 2026-09-16 night, which replaced "highest output speed")
/// therefore picks the same combination: every layer on the GPU measured
/// fastest at both, so no other combination qualifies to replace it. The rule
/// decides the micro-batch afterwards.
pub const FIT_COMBINATIONS: [(&str, u32); 6] = [
    ("f16", 1024),
    ("q8_0", 1024),
    ("f16", 512),
    ("q8_0", 512),
    ("f16", 256),
    ("q8_0", 256),
];

/// The combination to load with, from each combination's fit at one context:
/// the first that puts every layer on the GPU; otherwise the one with the most
/// layers on the GPU (the earlier on a tie). Expert placement (a mixture-of-
/// experts model) ranks below every layer on the GPU and above a whole-layer
/// spill. Among expert placements: the most layers on the GPU, then the f16
/// cache when a combination keeping those layers has it, then the fewest
/// expert blocks in RAM. Measured on a 30B mixture-of-experts model with 16K
/// tokens in context (generation / prompt reading, tok/s): f16 at the default
/// 1,024 MiB margin 49.3 / 1,211 (29 expert blocks in RAM); f16 at 256 MiB
/// 52.3 / 1,173 (26 blocks); q8_0 at 1,024 MiB 44.5 / 1,254 (26 blocks); q8_0
/// at 256 MiB 41.1 / 1,009 (24 blocks). The 8-bit cache cost this model 10%
/// of generation although it freed room for more experts, while a smaller f16
/// margin gained 6%; a q8_0 cache is what lets a dense model fit every layer
/// (14B at 16K: 13.1 → 32.7).
pub fn fastest_combination(results: &[((&'static str, u32), Fit)]) -> Option<((&'static str, u32), Fit)> {
    if let Some(all) = results.iter().find(|(_, fit)| *fit == Fit::AllLayers) {
        return Some(*all);
    }
    let experts: Vec<((&'static str, u32), i32, u32)> = results
        .iter()
        .filter_map(|(combo, fit)| match fit {
            Fit::ExpertsOnCpu { gpu_layers, blocks } => Some((*combo, *gpu_layers, *blocks)),
            _ => None,
        })
        .collect();
    if let Some(most_layers) = experts.iter().map(|(_, layers, _)| gpu_layer_rank(*layers)).max() {
        let keeps_most = |layers: i32| gpu_layer_rank(layers) == most_layers;
        let has_f16 = experts.iter().any(|((cache, _), layers, _)| keeps_most(*layers) && *cache == "f16");
        let best = experts
            .iter()
            .filter(|((cache, _), layers, _)| keeps_most(*layers) && (!has_f16 || *cache == "f16"))
            .fold(None::<&((&'static str, u32), i32, u32)>, |best, item| match best {
                Some(chosen) if chosen.2 <= item.2 => Some(chosen),
                _ => Some(item),
            });
        if let Some(&(combo, gpu_layers, blocks)) = best {
            return Some((combo, Fit::ExpertsOnCpu { gpu_layers, blocks }));
        }
    }
    results
        .iter()
        .filter_map(|(combo, fit)| match fit {
            Fit::Layers(layers) => Some((*combo, *layers)),
            _ => None,
        })
        .fold(None::<((&'static str, u32), i32)>, |best, (combo, layers)| match best {
            Some((_, best_layers)) if best_layers >= layers => best,
            _ => Some((combo, layers)),
        })
        .map(|(combo, layers)| (combo, Fit::Layers(layers)))
}

/// Expert blocks a larger micro-batch may move to RAM without a calibration:
/// one per 48 blocks. Measured on a 48-block mixture-of-experts model with a
/// 4,096-token prompt: 25 → 26 blocks in RAM cost 2.1% of generation, and later
/// steps 2.9-4.8% per block (26 → 28: -9.5%; 28 → 31: -10.6%). A model with
/// fewer, larger blocks gets none (one block is a larger share of it), and so
/// does a model whose block count is unknown.
pub fn free_expert_blocks(block_count: Option<u32>) -> u32 {
    block_count.map(|count| count / 48).unwrap_or(0)
}

/// The micro-batch that loads without a calibration use, from the fit of the
/// chosen cache/margin combination at 512 and at 1,024 (None when the second
/// probe failed). Owner speed rule (`speed_rule`): at most 5% of generation
/// for at least 5x as much prompt reading. 1,024 when:
/// - both keep every layer on the GPU (generation computes one token at a
///   time and does not use the micro-batch, so it loses nothing; measured on
///   a 4,096-token prompt: prompt reading +3.3% on a 4B, +1.1% on an 8B,
///   +0.5% on a 12B, generation unchanged, and the probe at 1,024 keeps the
///   chosen VRAM margin);
/// - both keep expert tensors in RAM, 1,024 keeps as many layers on the GPU,
///   and moves the experts of at most `free_expert_blocks` more blocks to RAM
///   (30B mixture-of-experts model, 4,096-token prompt: -2.1% generation,
///   +46% prompt reading);
/// - both spill whole layers and 1,024 keeps exactly as many layers on the
///   GPU. A whole dense layer costs far more than 5% of generation (27B at
///   32K: 57 → 61 layers was +36%, ~8% per layer; 14B at 16K: 41 → 44 → 45
///   layers was 13.1 → 17.5 → 19.9 tok/s), so losing even one breaks the rule.
///
/// Otherwise 512: the placement got worse in a way the rule does not allow,
/// or the larger size could not be probed.
pub fn default_micro_batch(at_512: Fit, at_1024: Option<Fit>, block_count: Option<u32>) -> u32 {
    let larger_is_free = match (at_512, at_1024) {
        (Fit::AllLayers, Some(Fit::AllLayers)) => true,
        (Fit::ExpertsOnCpu { gpu_layers, blocks }, Some(Fit::ExpertsOnCpu { gpu_layers: larger_layers, blocks: larger_blocks })) => {
            gpu_layer_rank(larger_layers) >= gpu_layer_rank(gpu_layers)
                && larger_blocks <= blocks.saturating_add(free_expert_blocks(block_count))
        }
        (Fit::Layers(layers), Some(Fit::Layers(larger))) => layers > 0 && larger == layers,
        _ => false,
    };
    if larger_is_free {
        BALANCED_MICRO_BATCH
    } else {
        DEFAULT_MICRO_BATCH
    }
}

/// Whether a calibrated micro-batch may replace the default at this load. The
/// calibration measured one change (from 512 to its chosen size, with
/// `measured_extra_blocks` more expert blocks in RAM) at the fit tool's default
/// cache and margin and its own context; this load may use another combination
/// or context, where the larger compute buffer can move more of the model. The
/// calibrated size is kept only when this load's change is no larger than the
/// measured one:
/// - 512 keeps every layer on the GPU here: the calibrated size must too;
/// - expert placements: no fewer layers on the GPU, and at most
///   `measured_extra_blocks` more expert blocks in RAM;
/// - whole-layer spills: no fewer layers on the GPU (a layer costs ~8%).
///
/// Otherwise `default_micro_batch` decides.
pub fn calibrated_micro_batch_fits(at_default: Fit, at_calibrated: Fit, measured_extra_blocks: u32) -> bool {
    match (at_default, at_calibrated) {
        (Fit::AllLayers, calibrated) => calibrated == Fit::AllLayers,
        (Fit::ExpertsOnCpu { gpu_layers, blocks }, Fit::ExpertsOnCpu { gpu_layers: calibrated_layers, blocks: calibrated_blocks }) => {
            gpu_layer_rank(calibrated_layers) >= gpu_layer_rank(gpu_layers)
                && calibrated_blocks <= blocks.saturating_add(measured_extra_blocks)
        }
        (Fit::Layers(layers), Fit::Layers(calibrated_layers)) => layers > 0 && calibrated_layers >= layers,
        _ => false,
    }
}

/// Load without mmap (`--load-mode none`) when the placement keeps weights in
/// RAM while using the GPU, and free RAM holds the whole model file plus
/// `PINNED_COPY_HEADROOM_MIB`. The weights kept in RAM then live in pinned
/// memory the GPU copies from directly for large prompt batches, instead of a
/// pageable file mapping. Measured on a 30B mixture-of-experts model with
/// experts partly in RAM (2,048-token prompt): prompt reading 2,079-2,160 →
/// 3,126-3,179 tok/s, generation 55-59 → 59-61; the pinned copy took 9.3 GB of
/// RAM for that placement. The copy is never larger than the file, so the file
/// size (every shard of a split set) bounds it. Start time measured on the same
/// model: 5.45-5.55 s with mmap, 5.55-5.64 s without (file already cached).
/// Rules that move tensors to another GPU do not count as weights in RAM.
pub fn load_without_mmap(fit: Fit, ram_available_bytes: u64, model_file_bytes: u64) -> bool {
    const MIB: u64 = 1_048_576;
    fit.keeps_weights_in_ram()
        && model_file_bytes > 0
        && ram_available_bytes >= model_file_bytes.saturating_add(PINNED_COPY_HEADROOM_MIB * MIB)
}

/// Bytes of weights a placement keeps in RAM, estimated from the model's size
/// on disk: the share of blocks whose experts stay there, or the share of
/// layers (blocks plus the output layer) left on the CPU. A block's experts
/// are counted as the whole block, so the estimate errs high for expert
/// placements. Half the file when the block count is unknown, as before.
pub fn weights_in_ram_bytes(fit: Fit, model_bytes: u64, block_count: Option<u32>) -> u64 {
    let share = |part: u64, whole: u64| -> u64 {
        let whole = whole.max(1);
        ((u128::from(model_bytes) * u128::from(part.min(whole))) / u128::from(whole)) as u64
    };
    match (fit, block_count.filter(|count| *count > 0)) {
        (Fit::AllLayers, _) => 0,
        (Fit::Layers(layers), _) if layers <= 0 => model_bytes,
        (_, None) => model_bytes / 2,
        (Fit::ExpertsOnCpu { blocks, .. }, Some(count)) => share(u64::from(blocks), u64::from(count)),
        (Fit::Layers(layers), Some(count)) => {
            let total = u64::from(count) + 1;
            share(total.saturating_sub(layers as u64), total)
        }
    }
}

/// `-ub N` for a micro-batch, and `-b N` when N is larger than the logical
/// batch the tools default to (which caps the micro-batch). Same spelling for
/// `llama-fit-params` and `llama-bench`.
pub fn micro_batch_args(micro_batch: u32) -> Vec<String> {
    let micro_batch = micro_batch.max(1);
    let mut args = vec!["-ub".to_string(), micro_batch.to_string()];
    if micro_batch > RUNTIME_DEFAULT_BATCH {
        args.extend(["-b".to_string(), micro_batch.to_string()]);
    }
    args
}

/// Arguments for one `llama-fit-params` probe.
pub fn probe_args(model: &std::path::Path, context: u32, cache_type: &str, margin_mib: u32, micro_batch: u32) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        model.display().to_string(),
        "-c".into(),
        context.to_string(),
        "-ctk".into(),
        cache_type.to_string(),
        "-ctv".into(),
        cache_type.to_string(),
        "-fitt".into(),
        margin_mib.to_string(),
    ];
    args.extend(micro_batch_args(micro_batch));
    args
}

/// Probe the runtime: how many layers of `model` fit at `context` tokens with
/// the given cache precision and micro-batch, leaving `margin_mib` of VRAM
/// free.
pub async fn probe(
    fit_tool: &std::path::Path,
    model: &std::path::Path,
    context: u32,
    cache_type: &str,
    margin_mib: u32,
    micro_batch: u32,
) -> Result<Fit, String> {
    let mut command = tokio::process::Command::new(fit_tool);
    command
        .args(probe_args(model, context, cache_type, margin_mib, micro_batch))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = fit_tool.parent() {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let child = command.spawn().map_err(|error| format!("could not start the fit probe: {error}"))?;
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| "the fit probe did not finish".to_string())?
        .map_err(|error| format!("the fit probe failed: {error}"))?;
    if !output.status.success() {
        return Err("the runtime could not read the model".into());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_fit_output(&stdout)?)
}

/// Read `llama-fit-params` output: `-c N -ngl K`, where `-ngl -1` means
/// nothing had to move, a smaller K means whole layers spill to the CPU, and
/// `-ot` rules mean expert tensors were moved instead of (or as well as)
/// layers. K is kept with the rules: see `Fit::ExpertsOnCpu`.
pub fn parse_fit_output(stdout: &str) -> Result<Fit, String> {
    let layers = fitted_gpu_layers(stdout).ok_or_else(|| format!("unexpected fit output: {}", stdout.trim()))?;
    let rules = tensor_overrides(stdout);
    if !rules.is_empty() {
        // Only rules that keep tensors in RAM count: on several GPUs the fit
        // also moves a partial layer to the next device (`=CUDA1`). One
        // `blk\.N\.` per block; a rule without a block number covers them all.
        let on_cpu: Vec<&String> = rules.iter().filter(|rule| rule.ends_with("=CPU")).collect();
        let blocks = if on_cpu.iter().any(|rule| !rule.contains("blk\\.")) {
            u32::MAX
        } else {
            on_cpu.iter().map(|rule| rule.matches("blk\\.").count() as u32).sum()
        };
        return Ok(Fit::ExpertsOnCpu { gpu_layers: layers, blocks });
    }
    Ok(if layers < 0 { Fit::AllLayers } else { Fit::Layers(layers) })
}

/// The `-ngl` value `llama-fit-params` printed.
pub fn fitted_gpu_layers(stdout: &str) -> Option<i32> {
    stdout
        .split_whitespace()
        .skip_while(|token| *token != "-ngl")
        .nth(1)
        .and_then(|value| value.parse::<i32>().ok())
}

/// The `-ot` rules `llama-fit-params` printed, one `pattern=buffer` per entry
/// (empty when it moved no tensors). The tool prints them comma-separated, as
/// llama-server takes them; llama-bench needs them joined with `;` instead,
/// because a comma there starts another test.
pub fn tensor_overrides(stdout: &str) -> Vec<String> {
    let Some((_, rest)) = stdout.split_once(" -ot ") else {
        return Vec::new();
    };
    let rest = rest.trim_start();
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(""),
        None => rest.split_whitespace().next().unwrap_or(""),
    };
    // A comma inside a pattern (a repetition such as `{1,3}`) leaves a piece
    // without `=`; it belongs to the piece after it.
    let mut rules = Vec::new();
    let mut pending = String::new();
    for piece in value.split(',') {
        if !pending.is_empty() {
            pending.push(',');
        }
        pending.push_str(piece);
        if piece.contains('=') {
            rules.push(std::mem::take(&mut pending));
        }
    }
    rules
}

/// Probes run at the same time while searching (owner decision 2026-09-17).
/// Every probe measures free VRAM with its own GPU context, so probes running
/// together can only see less free memory than a probe alone. A batched
/// search is therefore confirmed with probes run on their own before a load
/// uses it, and repeated one probe at a time if they disagree.
pub const FIT_PROBE_CONCURRENCY: usize = 4;

/// Bumped when the search or what it decides changes, so decisions remembered
/// by an older version are searched again.
pub const FIT_MEMORY_VERSION: u32 = 1;

/// More free VRAM than when a compromise was found, beyond which it is
/// searched again: the smallest margin step between combinations.
pub const FIT_MEMORY_VRAM_TOLERANCE_MIB: u64 = 256;

/// One `llama-fit-params` question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeRequest<'a> {
    pub context: u32,
    pub cache: &'a str,
    pub margin_mib: u32,
    pub micro_batch: u32,
}

/// Probe `requests` up to `concurrency` at a time, results in request order.
/// Stops at the first result `stop` accepts; probes still running then are
/// dropped, which kills their processes.
pub async fn probe_in_order(
    fit_tool: &std::path::Path,
    model: &std::path::Path,
    requests: &[ProbeRequest<'_>],
    concurrency: usize,
    stop: impl Fn(&Result<Fit, String>) -> bool,
) -> Vec<Result<Fit, String>> {
    use futures::stream::{self, StreamExt};
    // Owned futures: a borrowing closure held across the awaits below makes
    // the caller's task fail the Send bound tokio::spawn needs.
    let probes: Vec<_> = requests
        .iter()
        .map(|request| {
            let (fit_tool, model, cache) = (fit_tool.to_path_buf(), model.to_path_buf(), request.cache.to_string());
            let (context, margin_mib, micro_batch) = (request.context, request.margin_mib, request.micro_batch);
            async move { probe(&fit_tool, &model, context, &cache, margin_mib, micro_batch).await }
        })
        .collect();
    let mut results = Vec::with_capacity(probes.len());
    let mut pending = stream::iter(probes).buffered(concurrency.max(1));
    while let Some(result) = pending.next().await {
        let done = stop(&result);
        results.push(result);
        if done {
            break;
        }
    }
    results
}

/// Up to `count` evenly spread whole steps strictly between `low` and `high`.
fn spread(low: u32, high: u32, count: usize) -> Vec<u32> {
    let gap = high.saturating_sub(low);
    if gap <= 1 || count == 0 {
        return Vec::new();
    }
    if (gap - 1) as usize <= count {
        return (low + 1..high).collect();
    }
    let mut points: Vec<u32> = (1..=count as u64)
        .map(|index| low + (index * u64::from(gap) / (count as u64 + 1)) as u32)
        .filter(|point| *point > low && *point < high)
        .collect();
    points.dedup();
    points
}

/// The cache precision and margin combinations a load searches, in preference
/// order. A saved q8_0 cache preference keeps only the q8_0 ones.
pub fn fit_combinations(kv_preference: &str) -> Vec<(&'static str, u32)> {
    FIT_COMBINATIONS
        .iter()
        .copied()
        .filter(|(cache, _)| kv_preference != "q8_0" || *cache == "q8_0")
        .collect()
}

/// Everything a load takes from the fit search. Remembered per model, runtime
/// and request (`fit_memory_key`) so the next load only confirms it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FitDecision {
    pub requested: u32,
    pub context: u32,
    pub cache: String,
    pub margin: u32,
    pub micro_batch: Option<u32>,
    /// The fit at `micro_batch` (the runtime's default when None).
    pub fit: Fit,
    pub placement: String,
    pub placement_at_default: String,
    pub draft_head_dropped: bool,
    pub notes: Vec<String>,
    pub micro_batch_note: Option<String>,
    /// Free VRAM when the search ran, when it was known.
    pub free_vram_mib: Option<u64>,
}

/// What a load does with a remembered decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitReuse {
    /// One probe of the remembered setting; used when it gives the same fit.
    Confirm,
    /// A full search.
    Search,
}

/// Whether a remembered decision is confirmed with one probe or searched again.
/// A decision with the full request on the GPU and the most preferred cache and
/// margin cannot improve, so it is confirmed whatever VRAM is free now (the
/// probe catches less). A compromise (a smaller context, some of the model in
/// RAM, or less headroom than preferred) is searched again when noticeably
/// more VRAM is free than when it was found, or when that cannot be told.
pub fn fit_reuse(remembered: &FitDecision, free_vram_mib: Option<u64>, preferred: (&str, u32)) -> FitReuse {
    let improvable = remembered.context < remembered.requested
        || remembered.placement != "gpu"
        || (remembered.cache.as_str(), remembered.margin) != preferred;
    if !improvable {
        return FitReuse::Confirm;
    }
    match (free_vram_mib, remembered.free_vram_mib) {
        (Some(now), Some(then)) if now <= then + FIT_MEMORY_VRAM_TOLERANCE_MIB => FitReuse::Confirm,
        _ => FitReuse::Search,
    }
}

/// What a remembered fit decision depends on: the model file (name, bytes of
/// every shard, modification time), the fit tool (path and modification time,
/// so a rebuilt runtime searches again), the request (context, cache
/// preference, fit or requested-size mode, calibrated micro-batch, draft head)
/// and the GPU's total VRAM.
#[allow(clippy::too_many_arguments)]
pub fn fit_memory_key(
    model: &std::path::Path,
    fit_tool: &std::path::Path,
    requested: u32,
    kv_preference: &str,
    keeps_requested: bool,
    calibrated_micro_batch: Option<(u32, u32)>,
    draft_head: bool,
    vram_total_mib: Option<u64>,
) -> String {
    let modified = |path: &std::path::Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_secs())
            .unwrap_or(0)
    };
    let name = model.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    format!(
        "v{FIT_MEMORY_VERSION}|{name}|{}|{}|{}|{}|ctx{requested}|kv-{kv_preference}|{}|cal{:?}|head-{draft_head}|vram{:?}",
        crate::models::model_set_bytes(model),
        modified(model),
        fit_tool.display(),
        modified(fit_tool),
        if keeps_requested { "keep" } else { "fit" },
        calibrated_micro_batch,
        // Rounded: the same card can report its total a few MiB apart.
        vram_total_mib.map(|total| (total + 128) / 256 * 256)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake runtime whose GPU holds every layer up to `limit` tokens; counts probes.
    fn runtime(limit: u32, calls: std::rc::Rc<std::cell::Cell<u32>>) -> impl FnMut(Vec<u32>) -> std::future::Ready<Vec<Result<Fit, String>>> {
        move |contexts| {
            calls.set(calls.get() + contexts.len() as u32);
            std::future::ready(contexts.into_iter().map(|context| Ok(if context <= limit { Fit::AllLayers } else { Fit::Layers(40) })).collect())
        }
    }

    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    #[test]
    fn the_largest_full_gpu_context_is_found_in_few_probes() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let found = run(largest_full_gpu_context(32_768, 4_096, 1_024, 1, runtime(21_500, calls.clone()))).unwrap();
        assert_eq!(found, Some(21_504 - 1_024), "largest 1,024-token step at or under the limit");
        assert!(calls.get() <= 8, "{} probes", calls.get());
    }

    #[test]
    fn a_request_that_fits_is_kept_and_one_that_never_fits_is_none() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        assert_eq!(run(largest_full_gpu_context(32_768, 4_096, 1_024, 1, runtime(100_000, calls.clone()))).unwrap(), Some(32_768));
        assert_eq!(calls.get(), 1);
        assert_eq!(run(largest_full_gpu_context(32_768, 4_096, 1_024, 1, runtime(2_000, calls))).unwrap(), None);
    }

    #[test]
    fn the_fastest_combination_is_every_layer_on_the_gpu_then_the_most_layers() {
        let all = [(("f16", 1024), Fit::Layers(41)), (("q8_0", 1024), Fit::Layers(48)), (("f16", 512), Fit::AllLayers), (("q8_0", 512), Fit::AllLayers)];
        assert_eq!(fastest_combination(&all), Some((("f16", 512), Fit::AllLayers)), "the first with every layer, most headroom");
        let partial = [(("f16", 1024), Fit::Layers(57)), (("q8_0", 1024), Fit::Layers(62)), (("f16", 256), Fit::Layers(61)), (("q8_0", 256), Fit::Layers(62))];
        assert_eq!(fastest_combination(&partial), Some((("q8_0", 1024), Fit::Layers(62))), "most layers, earlier on a tie");
        assert_eq!(fastest_combination(&[]), None);
        let experts = |gpu_layers: i32, blocks: u32| Fit::ExpertsOnCpu { gpu_layers, blocks };
        let moe = [(("f16", 1024), experts(49, 28)), (("f16", 512), experts(49, 26)), (("f16", 256), experts(49, 24))];
        assert_eq!(fastest_combination(&moe), Some((("f16", 256), experts(49, 24))), "fewest expert blocks on the CPU");
        // The 30B at 16K: q8_0 at 256 MiB freed the most experts but was 14% slower than f16 at 1,024;
        // f16 at 256 MiB was the fastest measured.
        let measured = [
            (("f16", 1024), experts(49, 29)),
            (("q8_0", 1024), experts(49, 26)),
            (("f16", 512), experts(49, 27)),
            (("q8_0", 512), experts(49, 25)),
            (("f16", 256), experts(49, 26)),
            (("q8_0", 256), experts(49, 24)),
        ];
        assert_eq!(fastest_combination(&measured), Some((("f16", 256), experts(49, 26))), "f16 when it keeps as many layers");
        let q8_only = [(("q8_0", 1024), experts(49, 26)), (("q8_0", 256), experts(49, 24))];
        assert_eq!(fastest_combination(&q8_only), Some((("q8_0", 256), experts(49, 24))), "a saved q8_0 preference is honoured");
        // When even the dense weights do not fit, the tool lowers -ngl and
        // prints fewer rules: fewer rules there means fewer layers on the GPU.
        let small_gpu = [(("f16", 1024), experts(28, 27)), (("q8_0", 1024), experts(30, 29))];
        assert_eq!(fastest_combination(&small_gpu), Some((("q8_0", 1024), experts(30, 29))), "most layers on the GPU first");
    }

    #[test]
    fn fit_output_is_read_as_the_tool_prints_it() {
        assert_eq!(parse_fit_output("-c 8192 -ngl -1\n").unwrap(), Fit::AllLayers);
        assert_eq!(parse_fit_output("-c 17152 -ngl 41").unwrap(), Fit::Layers(41));
        let moe = r#"-c 4096 -ngl 49 -ot "blk\.24\.ffn_(gate|gate_up|down).*=CPU,blk\.25\.ffn_(up|down|gate_up|gate)_(ch|)exps=CPU""#;
        assert_eq!(parse_fit_output(moe).unwrap(), Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 2 });
        let every_block = r#"-c 4096 -ngl 99 -ot "\.ffn_(up|down|gate|gate_up)_(ch|)exps=CPU""#;
        assert_eq!(parse_fit_output(every_block).unwrap(), Fit::ExpertsOnCpu { gpu_layers: 99, blocks: u32::MAX });
        let second_gpu = r#"-c 4096 -ngl 49 -ts 30,19 -ot "blk\.29\.ffn_(gate|gate_up|down).*=CUDA1""#;
        assert_eq!(parse_fit_output(second_gpu).unwrap(), Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 0 }, "another GPU is not RAM");
        assert!(!parse_fit_output(second_gpu).unwrap().keeps_weights_in_ram());
        assert!(parse_fit_output("garbage").is_err());
    }

    #[test]
    fn probes_carry_the_micro_batch_and_raise_the_batch_above_its_default() {
        let has = |args: &[String], key: &str, value: &str| args.windows(2).any(|pair| pair[0] == key && pair[1] == value);
        let at_1024 = probe_args(std::path::Path::new("model.gguf"), 16_384, "q8_0", 512, 1024);
        assert!(has(&at_1024[..], "-ub", "1024"));
        assert!(has(&at_1024[..], "-c", "16384") && has(&at_1024[..], "-ctk", "q8_0") && has(&at_1024[..], "-ctv", "q8_0") && has(&at_1024[..], "-fitt", "512"));
        assert!(!at_1024.contains(&"-b".to_string()), "the default logical batch (2,048) already holds 1,024");
        let at_4096 = micro_batch_args(4096);
        assert_eq!(at_4096, vec!["-ub", "4096", "-b", "4096"]);
        assert_eq!(micro_batch_args(2048), vec!["-ub", "2048"]);
    }

    #[test]
    fn the_default_micro_batch_grows_only_when_the_speed_rule_allows_it() {
        let experts = |gpu_layers: i32, blocks: u32| Fit::ExpertsOnCpu { gpu_layers, blocks };
        let blocks_48 = Some(48);
        assert_eq!(default_micro_batch(Fit::AllLayers, Some(Fit::AllLayers), blocks_48), 1024, "every layer on the GPU at both");
        assert_eq!(default_micro_batch(Fit::AllLayers, Some(experts(49, 1)), blocks_48), 512, "the placement changed kind");
        assert_eq!(default_micro_batch(Fit::AllLayers, Some(Fit::Layers(47)), blocks_48), 512);
        assert_eq!(default_micro_batch(experts(49, 25), Some(experts(49, 26)), blocks_48), 1024, "one more expert block: -2% for +46%");
        assert_eq!(default_micro_batch(experts(49, 25), Some(experts(49, 25)), blocks_48), 1024);
        assert_eq!(default_micro_batch(experts(49, 25), Some(experts(49, 28)), blocks_48), 512, "three more blocks: -11% at 2,048");
        assert_eq!(default_micro_batch(experts(49, 47), Some(experts(49, u32::MAX)), blocks_48), 512, "a rule over every block is not one more");
        assert_eq!(default_micro_batch(experts(49, 25), Some(Fit::Layers(40)), blocks_48), 512);
        assert_eq!(default_micro_batch(experts(30, 29), Some(experts(28, 27)), blocks_48), 512, "fewer rules because two whole layers left the GPU");
        assert_eq!(default_micro_batch(experts(49, 12), Some(experts(49, 13)), Some(24)), 512, "on a 24-block model one block is a larger share");
        assert_eq!(default_micro_batch(experts(49, 12), Some(experts(49, 13)), None), 512, "unknown block count: no free block");
        assert_eq!(default_micro_batch(experts(95, 40), Some(experts(95, 41)), Some(94)), 1024, "one per 48 blocks");
        assert_eq!(default_micro_batch(Fit::Layers(41), Some(Fit::Layers(41)), blocks_48), 1024, "the same layers on the GPU");
        assert_eq!(default_micro_batch(Fit::Layers(41), Some(Fit::Layers(40)), blocks_48), 512, "a whole layer costs ~8% of generation");
        assert_eq!(default_micro_batch(Fit::Layers(0), Some(Fit::Layers(0)), blocks_48), 512, "nothing on the GPU");
        assert_eq!(default_micro_batch(Fit::AllLayers, None, blocks_48), 512, "the larger size could not be probed");
    }

    #[test]
    fn a_calibrated_micro_batch_is_kept_only_within_the_change_it_measured() {
        let experts = |gpu_layers: i32, blocks: u32| Fit::ExpertsOnCpu { gpu_layers, blocks };
        assert!(calibrated_micro_batch_fits(Fit::AllLayers, Fit::AllLayers, 0));
        assert!(!calibrated_micro_batch_fits(Fit::AllLayers, experts(49, 2), 3), "512 keeps every layer on the GPU here, so must the calibrated size");
        assert!(calibrated_micro_batch_fits(experts(49, 25), experts(49, 26), 1), "the one block the calibration measured");
        assert!(!calibrated_micro_batch_fits(experts(49, 25), experts(49, 28), 1), "more blocks than measured");
        assert!(!calibrated_micro_batch_fits(experts(30, 29), experts(28, 27), 1), "whole layers left the GPU");
        assert!(calibrated_micro_batch_fits(Fit::Layers(41), Fit::Layers(41), 0));
        assert!(!calibrated_micro_batch_fits(Fit::AllLayers, Fit::Layers(47), 0));
        assert!(!calibrated_micro_batch_fits(experts(49, 25), Fit::Layers(48), 5));
        assert!(!calibrated_micro_batch_fits(Fit::Layers(41), Fit::Layers(39), 0));
    }

    #[test]
    fn split_placements_draft_shorter_ngrams() {
        let experts = Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 25 };
        assert_eq!(ngram_draft_length(experts, "ngram-simple"), Some(24));
        assert_eq!(ngram_draft_length(Fit::Layers(41), "draft-mtp,ngram-simple"), Some(24));
        assert_eq!(ngram_draft_length(Fit::AllLayers, "ngram-simple"), None, "fully on the GPU the default was faster");
        assert_eq!(ngram_draft_length(Fit::Layers(0), "ngram-simple"), None, "CPU-only is not measured yet");
        assert_eq!(ngram_draft_length(experts, "none"), None);
        assert_eq!(ngram_draft_length(experts, "draft-mtp"), None);
    }

    #[test]
    fn weights_kept_in_ram_are_estimated_from_the_placement() {
        const GB: u64 = 1_000_000_000;
        let file = 18_560_000_000;
        let estimate = weights_in_ram_bytes(Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 25 }, file, Some(48));
        assert!(estimate > 9 * GB && estimate < 10 * GB, "25 of 48 blocks: {estimate}");
        assert_eq!(weights_in_ram_bytes(Fit::AllLayers, file, Some(48)), 0);
        assert_eq!(weights_in_ram_bytes(Fit::Layers(0), file, Some(48)), file);
        assert_eq!(weights_in_ram_bytes(Fit::Layers(41), 49 * GB, Some(48)), 8 * GB, "8 of 49 layers");
        assert_eq!(weights_in_ram_bytes(Fit::ExpertsOnCpu { gpu_layers: 49, blocks: u32::MAX }, file, Some(48)), file, "every block, at most the file");
        assert_eq!(weights_in_ram_bytes(Fit::Layers(41), file, None), file / 2, "unknown block count: half, as before");
    }

    #[test]
    fn loading_without_mmap_needs_weights_in_ram_and_room_for_their_copy() {
        const GIB: u64 = 1_073_741_824;
        let file = 18 * GIB;
        let experts = Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 25 };
        assert!(load_without_mmap(experts, 40 * GIB, file));
        assert!(load_without_mmap(Fit::Layers(41), 40 * GIB, file), "whole layers in RAM are copied too");
        assert!(!load_without_mmap(Fit::AllLayers, 40 * GIB, file), "nothing the GPU copies from stays in RAM");
        assert!(!load_without_mmap(Fit::Layers(0), 40 * GIB, file), "CPU-only loads are unchanged");
        assert!(!load_without_mmap(Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 0 }, 40 * GIB, file), "rules to another GPU keep nothing in RAM");
        assert!(load_without_mmap(experts, file + 4 * GIB, file), "exactly the file plus the headroom");
        assert!(!load_without_mmap(experts, file + 4 * GIB - 1, file));
        assert!(!load_without_mmap(experts, 40 * GIB, 0), "an unknown file size is not room");
    }

    #[test]
    fn expert_rules_are_read_one_per_entry() {
        let moe = r#"-c 4096 -ngl 49 -ot "blk\.24\.ffn_(gate|gate_up|down).*=CPU,blk\.25\.ffn_(up|down|gate_up|gate)_(ch|)exps=CPU""#;
        assert_eq!(
            tensor_overrides(moe),
            vec![r"blk\.24\.ffn_(gate|gate_up|down).*=CPU".to_string(), r"blk\.25\.ffn_(up|down|gate_up|gate)_(ch|)exps=CPU".to_string()]
        );
        assert_eq!(fitted_gpu_layers(moe), Some(49));
        assert_eq!(tensor_overrides("-c 4096 -ngl 49 -ot blk\\.1\\.ffn_up_exps=CPU\r\n"), vec![r"blk\.1\.ffn_up_exps=CPU".to_string()]);
        assert_eq!(tensor_overrides(r#"-c 4096 -ngl 49 -ot "blk\.(1|2){1,3}\.ffn_up_exps=CPU""#), vec![r"blk\.(1|2){1,3}\.ffn_up_exps=CPU".to_string()], "a comma inside a pattern");
        assert!(tensor_overrides("-c 8192 -ngl -1").is_empty());
        assert!(Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 3 }.uses_gpu() && Fit::Layers(1).uses_gpu() && Fit::AllLayers.uses_gpu());
        assert!(!Fit::Layers(0).uses_gpu());
    }

    #[test]
    fn a_small_request_is_its_own_floor() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        assert_eq!(run(largest_full_gpu_context(2_048, 4_096, 1_024, 1, runtime(1_000, calls))).unwrap(), None);
    }

    #[test]
    fn a_batched_search_finds_what_one_probe_at_a_time_finds_in_fewer_rounds() {
        use std::cell::Cell;
        use std::rc::Rc;
        for limit in [500, 4_095, 4_096, 5_000, 12_345, 21_500, 31_743, 32_767, 32_768, 40_000] {
            let one = run(largest_full_gpu_context(32_768, 4_096, 1_024, 1, runtime(limit, Rc::new(Cell::new(0))))).unwrap();
            for width in [2usize, 3, 4, 6] {
                let rounds = Rc::new(Cell::new(0));
                let counted = rounds.clone();
                let mut inner = runtime(limit, Rc::new(Cell::new(0)));
                let batched = run(largest_full_gpu_context(32_768, 4_096, 1_024, width, move |contexts: Vec<u32>| {
                    assert!(contexts.len() <= width, "{} probes in one round of width {width}", contexts.len());
                    counted.set(counted.get() + 1);
                    inner(contexts)
                }))
                .unwrap();
                assert_eq!(batched, one, "limit {limit}, width {width}");
                if width >= 4 {
                    assert!(rounds.get() <= 3, "limit {limit}, width {width}: {} rounds", rounds.get());
                }
            }
        }
    }

    #[test]
    fn spread_points_lie_strictly_between_and_evenly() {
        assert_eq!(spread(0, 100, 3), vec![25, 50, 75]);
        assert_eq!(spread(4, 6, 3), vec![5]);
        assert_eq!(spread(4, 8, 5), vec![5, 6, 7]);
        assert!(spread(3, 4, 2).is_empty());
        assert!(spread(3, 9, 0).is_empty());
    }

    fn decision(context: u32, cache: &str, margin: u32, placement: &str, free: Option<u64>) -> FitDecision {
        FitDecision {
            requested: 32_768,
            context,
            cache: cache.into(),
            margin,
            micro_batch: Some(1_024),
            fit: if placement == "gpu" { Fit::AllLayers } else { Fit::Layers(40) },
            placement: placement.into(),
            placement_at_default: placement.into(),
            draft_head_dropped: false,
            notes: vec!["why".into()],
            micro_batch_note: None,
            free_vram_mib: free,
        }
    }

    #[test]
    fn a_remembered_fit_is_confirmed_unless_more_vram_could_improve_it() {
        let preferred = ("f16", 1_024);
        // Full request, every layer, preferred setting: nothing to gain.
        let best = decision(32_768, "f16", 1_024, "gpu", Some(9_000));
        assert_eq!(fit_reuse(&best, Some(11_000), preferred), FitReuse::Confirm);
        assert_eq!(fit_reuse(&best, None, preferred), FitReuse::Confirm);
        // A smaller context: confirmed while free VRAM is about what it was.
        let smaller = decision(22_528, "q8_0", 256, "gpu", Some(10_000));
        assert_eq!(fit_reuse(&smaller, Some(10_000 + FIT_MEMORY_VRAM_TOLERANCE_MIB), preferred), FitReuse::Confirm);
        assert_eq!(fit_reuse(&smaller, Some(9_000), preferred), FitReuse::Confirm, "less free: the probe decides");
        assert_eq!(fit_reuse(&smaller, Some(10_000 + FIT_MEMORY_VRAM_TOLERANCE_MIB + 1), preferred), FitReuse::Search);
        assert_eq!(fit_reuse(&smaller, None, preferred), FitReuse::Search, "unknown: search");
        // Less headroom than preferred, or weights in RAM, are compromises too.
        assert_eq!(fit_reuse(&decision(32_768, "q8_0", 512, "gpu", Some(10_000)), Some(11_000), preferred), FitReuse::Search);
        assert_eq!(fit_reuse(&decision(32_768, "f16", 1_024, "hybrid", Some(10_000)), Some(11_000), preferred), FitReuse::Search);
        // A saved q8_0 preference makes q8_0 at 1,024 the best.
        assert_eq!(fit_reuse(&decision(32_768, "q8_0", 1_024, "gpu", None), None, ("q8_0", 1_024)), FitReuse::Confirm);
    }

    #[test]
    fn a_remembered_fit_round_trips_and_its_key_follows_what_it_depends_on() {
        let remembered = decision(22_528, "q8_0", 256, "gpu", Some(10_000));
        let text = serde_json::to_string(&remembered).unwrap();
        assert_eq!(serde_json::from_str::<FitDecision>(&text).unwrap(), remembered);
        let moe = FitDecision { fit: Fit::ExpertsOnCpu { gpu_layers: 49, blocks: 26 }, placement: "hybrid".into(), ..remembered };
        assert_eq!(serde_json::from_str::<FitDecision>(&serde_json::to_string(&moe).unwrap()).unwrap(), moe);

        let dir = std::env::temp_dir().join(format!("companion-fit-key-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("model.gguf");
        let tool = dir.join("fit-tool");
        std::fs::write(&model, b"weights").unwrap();
        std::fs::write(&tool, b"tool").unwrap();
        let key = |requested, kv: &str, keep, calibrated, head, vram| fit_memory_key(&model, &tool, requested, kv, keep, calibrated, head, vram);
        let base = key(32_768, "f16", false, None, false, Some(12_227));
        assert_eq!(base, key(32_768, "f16", false, None, false, Some(12_230)), "a few MiB of reported total is the same card");
        for other in [
            key(16_384, "f16", false, None, false, Some(12_227)),
            key(32_768, "q8_0", false, None, false, Some(12_227)),
            key(32_768, "f16", true, None, false, Some(12_227)),
            key(32_768, "f16", false, Some((1_024, 1)), false, Some(12_227)),
            key(32_768, "f16", false, None, true, Some(12_227)),
            key(32_768, "f16", false, None, false, Some(16_376)),
        ] {
            assert_ne!(base, other);
        }
        std::fs::write(&model, b"other weights").unwrap();
        assert_ne!(base, key(32_768, "f16", false, None, false, Some(12_227)), "a replaced model file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_q8_0_preference_searches_only_q8_0() {
        assert_eq!(fit_combinations("f16").len(), FIT_COMBINATIONS.len());
        assert_eq!(fit_combinations("f16")[0], ("f16", 1_024));
        assert!(fit_combinations("q8_0").iter().all(|(cache, _)| *cache == "q8_0"));
        assert_eq!(fit_combinations("q8_0")[0], ("q8_0", 1_024));
    }
}
