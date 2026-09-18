//! Model manager (§8–§16): registry, loading modes, auto-config.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// §12 compute placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeMode {
    Cpu,
    Gpu,
    Hybrid,
}

/// §16 capability registry entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelCapabilities {
    pub chat: bool,
    pub coding: bool,
    pub tool_calling: bool,
    pub json_output: bool,
    pub vision: bool,
    pub audio: bool,
}

/// §9 model metadata (mirrors metadata.json next to model.gguf).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub id: String,
    pub name: String,
    pub architecture: String,
    pub quantization: String,
    pub parameters: String,
    pub context_length: u32,
    pub vision: bool,
    pub tool_calling: bool,
    /// §111 reasoning capability. Never default true: the UI must not claim
    /// native reasoning a model does not have.
    #[serde(default)]
    pub supports_reasoning: bool,
    /// f16 KV-cache bytes per context token, from the GGUF's own layer/head
    /// shape (block_count × kv heads × (key + value length) × 2 bytes). Used
    /// to size the context to the GPU before loading. None when the header
    /// lacks the keys; sliding-window layers make this an overestimate.
    #[serde(default)]
    pub kv_bytes_per_token: Option<u64>,
    /// Weight file size in bytes at scan time (every shard of a split set).
    #[serde(default)]
    pub weights_bytes: Option<u64>,
    /// Transformer blocks (`<arch>.block_count`), from the GGUF header. The
    /// load path scales how many expert blocks a larger micro-batch may move
    /// to RAM by it. None when the header lacks the key.
    #[serde(default)]
    pub block_count: Option<u32>,
    /// Built-in next-token prediction layers (`<arch>.nextn_predict_layers`),
    /// the draft head llama-server uses with `--spec-type draft-mtp`. None or
    /// 0 when the file has none.
    #[serde(default)]
    pub draft_head_layers: Option<u32>,
    /// Restrictions this model's own chat template places on the message
    /// list. Always taken from the GGUF, never from metadata.json: it
    /// describes the template, and the template travels with the weights.
    #[serde(default, skip_deserializing)]
    pub chat_template: crate::inference::ChatTemplateShape,
    /// Why this file may not load or chat correctly, read from its structure.
    /// Empty when nothing is wrong. Never read from metadata.json.
    #[serde(default, skip_deserializing)]
    pub load_issues: Vec<String>,
    /// llama.cpp's built-in chat format for this architecture, set only when
    /// the file carries no chat template (see `builtin_chat_format`). Read
    /// from the GGUF, never from metadata.json.
    #[serde(default, skip_deserializing)]
    pub builtin_chat_format: Option<String>,
    /// Where `tool_calling` came from: "runtime" when llama.cpp reported the
    /// template's capabilities for this file, "template" when only the chat
    /// template's source suggests it. Filled in by the model list.
    #[serde(default, skip_deserializing)]
    pub tool_support_source: Option<String>,
    /// A fingerprint of the chat template the model is served with (its own,
    /// or the runtime's built-in format), so a tool check made with another
    /// template is known to be out of date.
    #[serde(default, skip_deserializing)]
    pub template_fingerprint: Option<String>,
    /// How the model calls tools, from `tooling.json` beside it.
    #[serde(default, skip_deserializing)]
    pub tooling: Option<crate::tooling::ToolingProfile>,
    /// Whether that check is current for this runtime and template.
    #[serde(default, skip_deserializing)]
    pub tooling_state: Option<crate::tooling::ProfileState>,
    #[serde(default)]
    pub projector_file: Option<String>,
    /// GGUF filename inside `dir`. Older metadata omits this and continues to
    /// use `model.gguf`; automatic discovery records the original filename.
    #[serde(default)]
    pub model_file: Option<String>,
    #[serde(default)]
    pub dir: PathBuf,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default = "default_loaded")]
    pub loaded: bool,
}

fn default_loaded() -> bool {
    false
}

impl ModelMetadata {
    /// Never blindly trust metadata.json (§9): sanity-check fields.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("model id is empty".into());
        }
        if self.id == "."
            || self.id.contains("..")
            || !self
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err("model id must use letters, digits, dots, dashes or underscores".into());
        }
        if self.context_length == 0 {
            return Err(format!("model {} has zero context_length", self.id));
        }
        if let Some(file) = &self.model_file {
            if !safe_gguf_filename(file) {
                return Err(format!("model {} has an unsafe model_file", self.id));
            }
        }
        if self
            .projector_file
            .as_deref()
            .is_some_and(|file| !safe_gguf_filename(file))
        {
            return Err(format!("model {} has an unsafe projector_file", self.id));
        }
        if self.vision && self.projector_file.is_none() {
            // Vision models need mmproj; warn but allow (backend re-checks).
        }
        Ok(())
    }

    pub fn gguf_path(&self) -> PathBuf {
        self.dir
            .join(self.model_file.as_deref().unwrap_or("model.gguf"))
    }

    /// Resolve the actual registered file, not a directory guessed from its
    /// display ID. In particular, a root-level GGUF must never delete a sibling
    /// model directory or the entire models folder.
    pub fn deletion_path(&self, models_root: &Path) -> Result<PathBuf, String> {
        self.validate()?;
        let root = models_root
            .canonicalize()
            .map_err(|e| format!("cannot resolve models folder: {e}"))?;
        let file = self
            .gguf_path()
            .canonicalize()
            .map_err(|e| format!("cannot resolve model file: {e}"))?;
        if file == root || !file.starts_with(&root) || !file.is_file() {
            return Err("model file is outside the configured models folder".into());
        }
        Ok(file)
    }
}

fn safe_gguf_filename(file: &str) -> bool {
    let path = Path::new(file);
    !file.contains(['/', '\\', ':'])
        && !path.is_absolute()
        && path.components().count() == 1
        && path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("gguf"))
}

/// §14 auto-config recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoConfig {
    pub gpu_layers_percent: u8,
    pub context: u32,
    pub batch: u32,
    pub kv_cache_gpu: bool,
    pub note: String,
}

pub fn auto_configure(model_gb: f64, vram_gb: f64, ram_gb: f64, requested_ctx: u32) -> AutoConfig {
    // Rough KV-cache estimate: ~0.5 MB/token for 8B-class at Q4; scale linearly.
    let kv_gb = (requested_ctx as f64) * 0.000_5;
    let total_need = model_gb + kv_gb;
    if total_need <= vram_gb {
        AutoConfig {
            gpu_layers_percent: 100,
            context: requested_ctx,
            batch: 512,
            kv_cache_gpu: true,
            note: format!("Fits in VRAM ({total_need:.1}/{vram_gb:.1} GB). Full GPU offload."),
        }
    } else if model_gb * 0.5 <= vram_gb && total_need <= vram_gb + ram_gb {
        let pct = ((vram_gb / total_need) * 100.0).clamp(5.0, 95.0) as u8;
        AutoConfig {
            gpu_layers_percent: pct,
            context: requested_ctx,
            batch: 512,
            kv_cache_gpu: false,
            note: format!(
                "Model exceeds VRAM. Recommended hybrid split: GPU {pct}% / CPU {}%.",
                100 - pct
            ),
        }
    } else {
        AutoConfig {
            gpu_layers_percent: 0,
            context: requested_ctx.min(8192),
            batch: 256,
            kv_cache_gpu: false,
            note: "Insufficient VRAM+RAM for full context; fell back to CPU with reduced context."
                .into(),
        }
    }
}

/// Single-loaded-model manager (§15). MVP keeps one primary text model resident.
#[derive(Debug, Default)]
pub struct ModelManager {
    models: HashMap<String, ModelMetadata>,
    current_id: Option<String>,
}

impl ModelManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, mut meta: ModelMetadata) -> Result<(), String> {
        meta.validate()?;
        meta.loaded = self.current_id.as_deref() == Some(meta.id.as_str());
        if meta.loaded
            && self
                .models
                .get(&meta.id)
                .is_some_and(|existing| existing.gguf_path() != meta.gguf_path())
        {
            return Err(format!(
                "loaded model '{}' changed its file; unload it before replacing it",
                meta.id
            ));
        }
        self.models.insert(meta.id.clone(), meta);
        Ok(())
    }

    pub fn list(&self) -> Vec<ModelMetadata> {
        let mut v: Vec<_> = self.models.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn get(&self, id: &str) -> Option<&ModelMetadata> {
        self.models.get(id)
    }

    pub fn current(&self) -> Option<&ModelMetadata> {
        self.current_id.as_ref().and_then(|id| self.models.get(id))
    }

    /// §15 switch: unload current, free, load new. Caller persists session first.
    pub fn switch_to(&mut self, id: &str) -> Result<ModelMetadata, String> {
        if !self.models.contains_key(id) {
            return Err(format!("unknown model '{id}'"));
        }
        if let Some(cur) = self.current_id.clone() {
            if let Some(m) = self.models.get_mut(&cur) {
                m.loaded = false;
            }
        }
        let m = self.models.get_mut(id).expect("checked");
        m.loaded = true;
        self.current_id = Some(id.to_string());
        Ok(m.clone())
    }

    pub fn unload_all(&mut self) {
        for m in self.models.values_mut() {
            m.loaded = false;
        }
        self.current_id = None;
    }

    /// Stage 4: unregister a model (e.g. after its directory was deleted).
    /// Make the registry match the models folder: register every model the
    /// scan found, and drop entries whose model file no longer exists.
    ///
    /// Scans used to register only, so a model folder deleted by hand stayed
    /// in the list for the life of the process, with a Load button for a file
    /// that was gone. The loaded model is kept even if its file vanished: the
    /// runtime still holds it open, and unloading is the user's action.
    /// Returns the ids removed, and a warning for each model that could not be
    /// registered.
    pub fn reconcile(&mut self, found: Vec<ModelMetadata>) -> (Vec<String>, Vec<String>) {
        let mut warnings = Vec::new();
        let found_ids: std::collections::HashSet<String> =
            found.iter().map(|model| model.id.clone()).collect();
        for model in found {
            if let Err(error) = self.register(model) {
                warnings.push(format!("skipping discovered model: {error}"));
            }
        }
        let current = self.current_id.clone();
        let removed: Vec<String> = self
            .models
            .values()
            .filter(|model| {
                !found_ids.contains(&model.id)
                    && current.as_deref() != Some(model.id.as_str())
                    && !model.gguf_path().is_file()
            })
            .map(|model| model.id.clone())
            .collect();
        for id in &removed {
            self.models.remove(id);
        }
        (removed, warnings)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if self.current_id.as_deref() == Some(id) {
            self.current_id = None;
        }
        self.models.remove(id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
struct GgufHeader {
    architecture: Option<String>,
    context_length: Option<u32>,
    /// What the embedded chat template can express. A template that has an
    /// `enable_thinking` switch or `<think>` markup carries native reasoning;
    /// one that renders `tools`/`tool_call` blocks supports tool calling. This
    /// is evidence from the model file itself, unlike a filename.
    template: TemplateHints,
    /// f16 KV bytes per token derived from the attention shape, when present.
    kv_bytes_per_token: Option<u64>,
    /// `<arch>.block_count`, when present.
    block_count: Option<u32>,
    /// `<arch>.nextn_predict_layers`, when present.
    draft_head_layers: Option<u32>,
    /// Facts that decide whether stock llama.cpp can load the file; see
    /// `load_issues`.
    layout: GgufLayout,
}

/// Structural facts read from the header, no weights involved.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GgufLayout {
    /// Vision/audio tower and projector tensors (`v.`, `a.`, `mm.`) inside a
    /// model file. llama.cpp keeps these in a separate projector (mmproj)
    /// file, and its text loader refuses tensors it does not claim.
    tower_tensors: u64,
    /// Entries in tokenizer.ggml.tokens.
    vocab_tokens: Option<u64>,
    /// Rows of token_embd.weight, which must equal the vocabulary.
    embedding_rows: Option<u64>,
    has_chat_template: bool,
}

/// Plain reasons a model file is unlikely to load or chat correctly in
/// upstream llama.cpp, from its structure alone. Advisory: the runtime's own
/// loader stays the authority, and a file is never refused on these grounds.
///
/// Every pattern here was observed in files taken from Ollama's registry,
/// which Ollama's patched runtime repairs in memory at load time: gemma3 and
/// gemma4 blobs with their towers packed in, a gemma3 tokenizer one entry
/// longer than its embedding, and no Jinja chat template (Ollama renders
/// prompts in Go). None of it is tied to a model name.
fn load_issues(layout: GgufLayout, architecture: Option<&str>) -> Vec<String> {
    let mut issues = Vec::new();
    if layout.tower_tensors > 0 {
        issues.push(format!(
            "{} vision/audio tensors are packed into this model file; llama.cpp expects them in a separate projector (mmproj) file and refuses the model. Files taken from Ollama's registry are packaged this way; use the upstream GGUF.",
            layout.tower_tensors
        ));
    }
    if let (Some(tokens), Some(rows)) = (layout.vocab_tokens, layout.embedding_rows) {
        if tokens != rows {
            issues.push(format!(
                "The tokenizer lists {tokens} tokens but the embedding has {rows} rows; llama.cpp refuses the mismatch."
            ));
        }
    }
    // Without a template the load uses the architecture's built-in format
    // when llama.cpp has one; only files without either are at risk.
    if !layout.has_chat_template && architecture.and_then(builtin_chat_format).is_none() {
        issues.push(
            "The file carries no chat template, so the runtime falls back to a generic format this model was not trained on; replies may be garbled or echo the prompt.".into(),
        );
    }
    issues
}

/// llama.cpp's built-in chat format for a GGUF architecture, for files that
/// carry no chat template. Without a template llama-server renders every
/// conversation as ChatML; a model trained on another format then never
/// produces its own end-of-turn token (measured: a Gemma 3 4B file without a
/// template ran to the 8,192-token output limit and invented conversations).
///
/// Only architectures whose instruction-tuned releases all use one format are
/// listed. `llama`, `phi3`, `deepseek2`, `chatglm`, `glm4`, `granite`, `minicpm`,
/// `cohere2` and `grok` each cover releases with different formats, so a
/// file of those architectures keeps the load issue instead of a guess.
/// "chatml" is the runtime's own fallback, so it needs no launch argument.
/// The names are llama.cpp's (`llm_chat_template` in llama-chat.cpp); the
/// load path checks that the installed runtime lists one before using it.
pub fn builtin_chat_format(architecture: &str) -> Option<&'static str> {
    match architecture {
        "gemma" | "gemma2" | "gemma3" | "gemma3n" => Some("gemma"),
        "qwen2" | "qwen2moe" | "qwen3" | "qwen3moe" => Some("chatml"),
        "llama4" => Some("llama4"),
        "command-r" => Some("command-r"),
        "gpt-oss" => Some("gpt-oss"),
        "hunyuan-moe" => Some("hunyuan-moe"),
        "hunyuan-dense" => Some("hunyuan-dense"),
        "seed_oss" => Some("seed_oss"),
        _ => None,
    }
}

/// The built-in format a file without a template is loaded with; None when it
/// has its own template or its architecture has no single built-in format.
fn fallback_chat_format(header: &GgufHeader) -> Option<String> {
    if header.layout.has_chat_template {
        return None;
    }
    header.architecture.as_deref().and_then(builtin_chat_format).map(str::to_string)
}

/// Attention shape keys needed to size the KV cache exactly.
#[derive(Debug, Default)]
struct AttentionShape {
    block_count: Option<u64>,
    head_count: Option<u64>,
    head_count_kv: Option<u64>,
    embedding_length: Option<u64>,
    key_length: Option<u64>,
    value_length: Option<u64>,
}

impl AttentionShape {
    fn kv_bytes_per_token(&self) -> Option<u64> {
        let layers = self.block_count?;
        let kv_heads = self.head_count_kv.or(self.head_count)?;
        let head_dim = self.key_length.or_else(|| {
            Some(self.embedding_length? / self.head_count?.max(1))
        })?;
        let value_dim = self.value_length.unwrap_or(head_dim);
        let bytes = layers
            .checked_mul(kv_heads)?
            .checked_mul(head_dim.checked_add(value_dim)?)?
            .checked_mul(2)?;
        (bytes > 0).then_some(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TemplateHints {
    thinking: bool,
    tools: bool,
    /// `tooling::template_fingerprint` of the template text.
    fingerprint: Option<String>,
    /// What the template refuses outright. Default permissive: a template is
    /// only called strict when it raises on the shape itself.
    shape: crate::inference::ChatTemplateShape,
}

impl Default for TemplateHints {
    fn default() -> Self {
        Self {
            thinking: false,
            tools: false,
            fingerprint: None,
            shape: crate::inference::ChatTemplateShape::default(),
        }
    }
}

/// What a template's own `raise_exception` calls refuse. Gemma 2 raises
/// "System role not supported" and "Conversation roles must alternate
/// user/assistant/..."; llama-server turns both into a 400 with no tokens
/// generated. Reading the refusal text is exact, where guessing from the
/// model's name is not: repacks and finetunes rename freely.
fn template_shape(lower: &str) -> crate::inference::ChatTemplateShape {
    let mut shape = crate::inference::ChatTemplateShape::default();
    for (index, _) in lower.match_indices("raise_exception") {
        let message: String = lower[index..].chars().take(160).collect();
        let message = message.split("')").next().unwrap_or(&message);
        if message.contains("system") {
            shape.system_role = false;
        }
        if message.contains("alternate") {
            shape.strict_alternation = true;
        }
    }
    shape
}

fn template_hints(template: &str) -> TemplateHints {
    let lower = template.to_ascii_lowercase();
    TemplateHints {
        thinking: lower.contains("enable_thinking")
            || lower.contains("<think>")
            || lower.contains("reasoning_effort")
            || lower.contains("thinking_mode"),
        tools: template_handles_tools(&lower),
        fingerprint: Some(crate::tooling::template_fingerprint(template)),
        shape: template_shape(&lower),
    }
}

/// The template renders tools or tool calls: a `tool_call(s)` field, or the
/// identifier `tools` used inside a Jinja tag. The word "tools" in prose a
/// template carries is not evidence (the previous test counted it).
fn template_handles_tools(lower: &str) -> bool {
    if lower.contains("tool_call") {
        return true;
    }
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (open, close) in [("{%", "%}"), ("{{", "}}")] {
        let mut rest = lower;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len()..];
            let Some(end) = after.find(close) else { break };
            let tag = &after[..end];
            let mut search = tag;
            while let Some(at) = search.find("tools") {
                let before = search[..at].chars().next_back();
                let next = search[at + "tools".len()..].chars().next();
                if !before.is_some_and(is_ident) && !next.is_some_and(is_ident) {
                    return true;
                }
                search = &search[at + "tools".len()..];
            }
            rest = &after[end + close.len()..];
        }
    }
    false
}

/// A multimodal projector next to the weights. Only an unambiguous single
/// candidate is paired automatically; two projectors need metadata.json.
fn projector_in(dir: &Path) -> Option<String> {
    let mut candidates: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".gguf") && lower.contains("mmproj")
        })
        .collect();
    candidates.sort();
    (candidates.len() == 1).then(|| candidates.remove(0))
}

type HeaderCache = HashMap<
    PathBuf,
    (
        u64,
        Option<std::time::SystemTime>,
        Result<GgufHeader, String>,
    ),
>;

/// Only metadata is read, never multi-gigabyte weights. Unchanged files reuse
/// the parsed header even when the UI refreshes its model list every few seconds.
fn gguf_header(path: &Path) -> Result<GgufHeader, String> {
    static CACHE: OnceLock<Mutex<HeaderCache>> = OnceLock::new();
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let stamp = (metadata.len(), metadata.modified().ok());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(entries) = cache.lock() {
        if let Some((size, modified, result)) = entries.get(path) {
            if (*size, *modified) == stamp {
                return result.clone();
            }
        }
    }
    let result = parse_gguf_header(path).map_err(|e| format!("invalid or incomplete GGUF: {e}"));
    if let Ok(mut entries) = cache.lock() {
        if entries.len() >= 512 {
            entries.clear();
        }
        entries.insert(path.to_path_buf(), (stamp.0, stamp.1, result.clone()));
    }
    result
}

fn parse_gguf_header(path: &Path) -> std::io::Result<GgufHeader> {
    use std::io::{Error, ErrorKind};
    fn invalid(message: &str) -> Error {
        Error::new(ErrorKind::InvalidData, message)
    }
    fn u32_value(reader: &mut impl Read) -> std::io::Result<u32> {
        let mut bytes = [0; 4];
        reader.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64_value(reader: &mut impl Read) -> std::io::Result<u64> {
        let mut bytes = [0; 8];
        reader.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn string(reader: &mut impl Read) -> std::io::Result<String> {
        let count = u64_value(reader)?;
        if count > 8 * 1024 * 1024 {
            return Err(invalid("metadata string too large"));
        }
        let mut bytes = vec![0; count as usize];
        reader.read_exact(&mut bytes)?;
        String::from_utf8(bytes).map_err(|_| invalid("metadata string is not UTF-8"))
    }
    fn skip(reader: &mut (impl Read + Seek), count: u64, length: u64) -> std::io::Result<()> {
        let end = reader
            .stream_position()?
            .checked_add(count)
            .ok_or_else(|| invalid("metadata overflow"))?;
        if end > length || end > 128 * 1024 * 1024 {
            return Err(invalid("metadata exceeds file or scan limit"));
        }
        reader.seek_relative(count as i64)?;
        Ok(())
    }
    fn skip_value(
        reader: &mut (impl Read + Seek),
        kind: u32,
        length: u64,
        array: bool,
    ) -> std::io::Result<()> {
        let fixed_size = match kind {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4 | 5 | 6 => 4,
            10 | 11 | 12 => 8,
            _ => 0,
        };
        if fixed_size > 0 {
            return skip(reader, fixed_size, length);
        }
        match kind {
            8 => {
                let count = u64_value(reader)?;
                skip(reader, count, length)
            }
            9 if !array => {
                let element = u32_value(reader)?;
                let count = u64_value(reader)?;
                if count > 2_000_000 {
                    return Err(invalid("metadata array too large"));
                }
                let size = match element {
                    0 | 1 | 7 => 1,
                    2 | 3 => 2,
                    4 | 5 | 6 => 4,
                    10 | 11 | 12 => 8,
                    _ => 0,
                };
                if size > 0 {
                    skip(reader, count * size, length)
                } else {
                    for _ in 0..count {
                        skip_value(reader, element, length, true)?;
                    }
                    Ok(())
                }
            }
            _ => Err(invalid("unsupported metadata value type")),
        }
    }
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let mut reader = std::io::BufReader::new(file);
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(invalid("missing GGUF signature"));
    }
    if !matches!(u32_value(&mut reader)?, 2 | 3) {
        return Err(invalid("unsupported GGUF version"));
    }
    let tensor_count = u64_value(&mut reader)?;
    if tensor_count == 0 || tensor_count > 100_000 {
        return Err(invalid("invalid model tensor count"));
    }
    let count = u64_value(&mut reader)?;
    if count > 100_000 {
        return Err(invalid("too many metadata keys"));
    }
    let mut header = GgufHeader::default();
    let mut contexts = Vec::new();
    let mut shape: Vec<(String, u64)> = Vec::new();
    let mut alignment = 32u64;
    const SHAPE_SUFFIXES: [&str; 7] = [
        ".nextn_predict_layers",
        ".block_count",
        ".attention.head_count",
        ".attention.head_count_kv",
        ".embedding_length",
        ".attention.key_length",
        ".attention.value_length",
    ];
    for _ in 0..count {
        if reader.stream_position()? > 128 * 1024 * 1024 {
            return Err(invalid("metadata scan limit exceeded"));
        }
        let key = string(&mut reader)?;
        let kind = u32_value(&mut reader)?;
        if key == "general.architecture" && kind == 8 {
            header.architecture = Some(string(&mut reader)?);
        } else if key == "tokenizer.chat_template" && kind == 8 {
            header.template = template_hints(&string(&mut reader)?);
            header.layout.has_chat_template = true;
        } else if key == "tokenizer.ggml.tokens" && kind == 9 {
            let element = u32_value(&mut reader)?;
            let count = u64_value(&mut reader)?;
            if count > 2_000_000 {
                return Err(invalid("metadata array too large"));
            }
            for _ in 0..count {
                skip_value(&mut reader, element, length, true)?;
            }
            header.layout.vocab_tokens = Some(count);
        } else if key == "general.alignment" && kind == 4 {
            alignment = u32_value(&mut reader)? as u64;
            if alignment == 0 || alignment > 4096 || !alignment.is_power_of_two() {
                return Err(invalid("unsupported tensor alignment"));
            }
        } else if key.ends_with(".context_length") && matches!(kind, 4 | 10) {
            let value = if kind == 4 {
                u32_value(&mut reader)? as u64
            } else {
                u64_value(&mut reader)?
            };
            if let Ok(value) = u32::try_from(value) {
                if value > 0 {
                    contexts.push((key, value));
                }
            }
        } else if SHAPE_SUFFIXES.iter().any(|suffix| key.ends_with(suffix))
            && matches!(kind, 4 | 10)
        {
            let value = if kind == 4 {
                u32_value(&mut reader)? as u64
            } else {
                u64_value(&mut reader)?
            };
            shape.push((key, value));
        } else if key.ends_with(".attention.head_count_kv") && kind == 9 {
            // Per-layer KV head counts (some hybrid architectures): take the
            // largest so the cache estimate stays conservative.
            let element = u32_value(&mut reader)?;
            let count = u64_value(&mut reader)?;
            if count > 2_000_000 {
                return Err(invalid("metadata array too large"));
            }
            if element == 4 {
                let mut largest = 0u64;
                for _ in 0..count {
                    largest = largest.max(u32_value(&mut reader)? as u64);
                }
                shape.push((key, largest));
            } else {
                for _ in 0..count {
                    skip_value(&mut reader, element, length, true)?;
                }
            }
        } else {
            skip_value(&mut reader, kind, length, false)?;
        }
    }
    if let Some(architecture) = &header.architecture {
        header.context_length = contexts
            .into_iter()
            .find(|(key, _)| key == &format!("{architecture}.context_length"))
            .map(|(_, value)| value);
        let lookup = |suffix: &str| {
            shape
                .iter()
                .find(|(key, _)| key == &format!("{architecture}{suffix}"))
                .map(|(_, value)| *value)
        };
        header.block_count = lookup(".block_count").and_then(|value| u32::try_from(value).ok());
        header.draft_head_layers = lookup(".nextn_predict_layers").and_then(|value| u32::try_from(value).ok());
        header.kv_bytes_per_token = AttentionShape {
            block_count: lookup(".block_count"),
            head_count: lookup(".attention.head_count"),
            head_count_kv: lookup(".attention.head_count_kv"),
            embedding_length: lookup(".embedding_length"),
            key_length: lookup(".attention.key_length"),
            value_length: lookup(".attention.value_length"),
        }
        .kv_bytes_per_token();
    }
    // Validate tensor locations without reading weights. A file whose header
    // finished downloading but whose tensor data is missing is not ready yet.
    let mut last_offset = 0;
    for _ in 0..tensor_count {
        let name = string(&mut reader)?;
        let dimensions = u32_value(&mut reader)?;
        if !(1..=4).contains(&dimensions) {
            return Err(invalid("invalid tensor dimensions"));
        }
        if name.starts_with("v.") || name.starts_with("a.") || name.starts_with("mm.") {
            header.layout.tower_tensors += 1;
        }
        for axis in 0..dimensions {
            let size = u64_value(&mut reader)?;
            if size == 0 {
                return Err(invalid("empty tensor dimension"));
            }
            // GGUF stores token_embd as [embedding width, vocabulary rows].
            if axis == 1 && name == "token_embd.weight" {
                header.layout.embedding_rows = Some(size);
            }
        }
        let _element_type = u32_value(&mut reader)?;
        let offset = u64_value(&mut reader)?;
        if offset % alignment != 0 {
            return Err(invalid("unaligned tensor offset"));
        }
        last_offset = last_offset.max(offset);
        if reader.stream_position()? > 128 * 1024 * 1024 {
            return Err(invalid("tensor table scan limit exceeded"));
        }
    }
    let position = reader.stream_position()?;
    let data_start = position
        .checked_add(alignment - 1)
        .ok_or_else(|| invalid("tensor offset overflow"))?
        / alignment
        * alignment;
    if data_start
        .checked_add(last_offset)
        .is_none_or(|offset| offset >= length)
    {
        return Err(invalid(
            "tensor data is missing; download may be incomplete",
        ));
    }
    Ok(header)
}

fn is_model_gguf(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("gguf"))
        && !path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.to_ascii_lowercase().contains("mmproj"))
}

fn split_pattern() -> &'static regex::Regex {
    static SPLIT: OnceLock<regex::Regex> = OnceLock::new();
    SPLIT.get_or_init(|| regex::Regex::new(r"(?i)^(.*)-(\d{5})-of-(\d{5})\.gguf$").unwrap())
}

/// Bytes of a model's weights on disk: every shard of a split set (llama.cpp
/// loads a set through its first shard, `name-00001-of-000NN.gguf`), or the
/// single file. 0 when nothing can be read.
pub fn model_set_bytes(path: &Path) -> u64 {
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    let Some(parts) = split_pattern().captures(name) else {
        return std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    };
    let count = parts[3].parse::<u32>().unwrap_or(0).min(1024);
    (1..=count)
        .map(|number| path.with_file_name(format!("{}-{number:05}-of-{count:05}.gguf", &parts[1])))
        .filter_map(|shard| std::fs::metadata(shard).ok())
        .map(|meta| meta.len())
        .sum()
}

/// llama.cpp loads a split set through its first shard. Registering every shard
/// as a separate model produces broken choices, so only expose complete sets.
fn complete_model_file(path: &Path) -> Result<bool, String> {
    let pattern = split_pattern();
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    if let Some(parts) = pattern.captures(name) {
        let index = parts[2].parse::<u32>().unwrap_or(0);
        let count = parts[3].parse::<u32>().unwrap_or(0);
        if index != 1 {
            return Ok(false);
        }
        if count == 0 || count > 1024 {
            return Err("invalid split model shard count".into());
        }
        for number in 1..=count {
            let sibling =
                path.with_file_name(format!("{}-{number:05}-of-{count:05}.gguf", &parts[1]));
            if !sibling
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() >= 24)
            {
                return Err(format!(
                    "split model is incomplete: missing {}",
                    sibling.display()
                ));
            }
        }
    }
    Ok(true)
}

fn gguf_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_model_gguf(path))
        .collect();
    files.sort_by_key(|path| path.file_name().map(|v| v.to_os_string()));
    files.sort_by_key(|path| {
        path.file_name()
            .and_then(|v| v.to_str())
            .map(|v| !v.eq_ignore_ascii_case("model.gguf"))
            .unwrap_or(true)
    });
    files
}

fn safe_id(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn infer_quantization(name: &str) -> String {
    static QUANT: OnceLock<regex::Regex> = OnceLock::new();
    QUANT
        .get_or_init(|| {
            regex::Regex::new(r"(?i)(?:^|[-_. ])((?:IQ|Q)\d[A-Z0-9_]*|BF16|F16|F32)(?:$|[-. ])")
                .unwrap()
        })
        .captures_iter(name)
        .last()
        .map(|capture| capture[1].to_ascii_uppercase())
        .unwrap_or_else(|| "unknown".into())
}

fn infer_parameters(name: &str) -> String {
    name.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.')
        .find_map(|part| {
            let lower = part.to_ascii_lowercase();
            let number = lower.strip_suffix('b')?;
            (!number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit() || ch == '.'))
                .then(|| format!("{}B", number.to_ascii_uppercase()))
        })
        .unwrap_or_else(|| "unknown".into())
}

fn inferred_name(stem: &str, quantization: &str) -> String {
    let upper = stem.to_ascii_uppercase();
    let base = upper
        .rfind(quantization)
        .map(|index| &stem[..index])
        .unwrap_or(stem)
        .trim_matches(['-', '_', ' ']);
    let mut words = Vec::new();
    for (index, word) in base
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .enumerate()
    {
        let lower = word.to_ascii_lowercase();
        let pretty = if lower.ends_with('b')
            && lower[..lower.len() - 1]
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch == '.')
        {
            lower.to_ascii_uppercase()
        } else if lower == "it" {
            "IT".into()
        } else if index == 0 {
            let mut chars = lower.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        } else {
            lower
        };
        words.push(pretty);
    }
    format!("{} ({quantization})", words.join(" "))
}

fn inferred_metadata(
    folder: &std::path::Path,
    gguf: &std::path::Path,
    id_hint: &str,
    header: &GgufHeader,
) -> ModelMetadata {
    let stem = gguf.file_stem().and_then(|v| v.to_str()).unwrap_or(id_hint);
    let id = {
        let candidate = safe_id(id_hint);
        if candidate.is_empty() {
            safe_id(stem)
        } else {
            candidate
        }
    };
    let quantization = infer_quantization(stem);
    let parameters = infer_parameters(stem);
    let projector_file = projector_in(folder);
    ModelMetadata {
        id,
        name: inferred_name(stem, &quantization),
        architecture: header
            .architecture
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        quantization,
        parameters,
        context_length: header.context_length.unwrap_or(4096),
        vision: projector_file.is_some(),
        // The chat template embedded in the file is the evidence, never the
        // filename: a template with a thinking switch declares it.
        tool_calling: header.template.tools,
        supports_reasoning: header.template.thinking,
        chat_template: header.template.shape,
        load_issues: load_issues(header.layout, header.architecture.as_deref()),
        builtin_chat_format: fallback_chat_format(header),
        tool_support_source: None,
        template_fingerprint: served_template_fingerprint(header),
        tooling: None,
        tooling_state: None,
        kv_bytes_per_token: header.kv_bytes_per_token,
        weights_bytes: Some(model_set_bytes(gguf)).filter(|bytes| *bytes > 0),
        block_count: header.block_count,
        draft_head_layers: header.draft_head_layers,
        projector_file: projector_file.clone(),
        model_file: gguf
            .file_name()
            .and_then(|v| v.to_str())
            .map(str::to_string),
        dir: folder.to_path_buf(),
        capabilities: ModelCapabilities {
            chat: true,
            tool_calling: header.template.tools,
            vision: projector_file.is_some(),
            ..ModelCapabilities::default()
        },
        loaded: false,
    }
}

/// The template a model is served with: its own, or for a file without one
/// the runtime's built-in format that the load path selects.
fn served_template_fingerprint(header: &GgufHeader) -> Option<String> {
    header
        .template
        .fingerprint
        .clone()
        .or_else(|| fallback_chat_format(header).map(|format| format!("built-in {format}")))
}

/// Scan model folders and root-level GGUF files. A metadata.json remains the
/// authoritative description, but a bare GGUF is immediately usable with
/// conservative inferred defaults instead of being silently ignored.
pub fn scan_models_dir(dir: &std::path::Path) -> (Vec<ModelMetadata>, Vec<String>) {
    let mut found = vec![];
    let mut warnings = vec![];
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (found, warnings),
        Err(e) => {
            warnings.push(format!("cannot list models dir {}: {e}", dir.display()));
            return (found, warnings);
        }
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let entry_path = entry.path();
        if entry_path.is_file() {
            if is_model_gguf(&entry_path) {
                let hint = entry_path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("model");
                if let Some(header) = checked_header(dir, &entry_path, &mut warnings) {
                    found.push(inferred_metadata(dir, &entry_path, hint, &header));
                }
            }
            continue;
        }
        if !entry_path.is_dir() {
            continue;
        }
        let meta_path = entry_path.join("metadata.json");
        let ggufs = gguf_files(&entry_path);
        if !meta_path.is_file() {
            let candidates: Vec<_> = ggufs
                .iter()
                .filter_map(|gguf| {
                    checked_header(dir, gguf, &mut warnings).map(|header| (gguf, header))
                })
                .collect();
            for (gguf, header) in &candidates {
                let folder_name = entry_path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("model");
                let hint = if candidates.len() == 1 {
                    folder_name.to_string()
                } else {
                    format!(
                        "{}-{}",
                        folder_name,
                        gguf.file_stem().and_then(|v| v.to_str()).unwrap_or("model")
                    )
                };
                found.push(inferred_metadata(&entry_path, gguf, &hint, header));
            }
            continue;
        }
        match std::fs::read_to_string(&meta_path) {
            Ok(text) => match serde_json::from_str::<ModelMetadata>(&text) {
                Ok(mut m) => {
                    // Pin dir to the discovered folder; never trust the file's dir (§9).
                    m.dir = entry.path();
                    m.loaded = false;
                    if let Err(e) = m.validate() {
                        warnings.push(format!("{}: invalid metadata: {e}", meta_path.display()));
                        continue;
                    }
                    // Derive capability defaults from flags when capabilities are absent.
                    if !m.capabilities.chat {
                        m.capabilities.chat = true;
                    }
                    if !m.gguf_path().is_file() {
                        // An explicit model_file is a binding, not a hint. Do
                        // not silently load different weights after deletion.
                        let candidates: Vec<_> = ggufs
                            .iter()
                            .filter(|file| complete_model_file(file).unwrap_or(false))
                            .collect();
                        if m.model_file.is_none() && candidates.len() == 1 {
                            let gguf = candidates[0];
                            m.model_file = gguf
                                .file_name()
                                .and_then(|v| v.to_str())
                                .map(str::to_string);
                        } else {
                            warnings.push(format!(
                                "{}: selected GGUF is missing or ambiguous; set model_file in metadata.json",
                                entry_path.display()
                            ));
                            continue;
                        }
                    }
                    let Some(header) = checked_header(dir, &m.gguf_path(), &mut warnings) else {
                        continue;
                    };
                    m.builtin_chat_format = fallback_chat_format(&header);
                    m.template_fingerprint = served_template_fingerprint(&header);
                    m.load_issues = load_issues(header.layout, header.architecture.as_deref());
                    if let Some(architecture) = header.architecture {
                        m.architecture = architecture;
                    }
                    if let Some(context) = header.context_length {
                        m.context_length = m.context_length.min(context);
                    }
                    // The file's template can only add capabilities the JSON
                    // omitted; it never removes an explicit declaration.
                    m.supports_reasoning |= header.template.thinking;
                    m.tool_calling |= header.template.tools;
                    m.chat_template = header.template.shape;
                    m.kv_bytes_per_token = header.kv_bytes_per_token;
                    m.block_count = header.block_count;
                    m.draft_head_layers = header.draft_head_layers;
                    m.weights_bytes = Some(model_set_bytes(&m.gguf_path())).filter(|bytes| *bytes > 0);
                    if m.projector_file.is_none() {
                        if let Some(projector) = projector_in(&entry_path) {
                            m.projector_file = Some(projector);
                            m.vision = true;
                        }
                    }
                    m.capabilities.tool_calling |= m.tool_calling;
                    m.tool_calling |= m.capabilities.tool_calling;
                    m.capabilities.vision |= m.vision;
                    m.vision |= m.capabilities.vision;
                    found.push(m);
                }
                Err(e) => warnings.push(format!("{}: bad JSON: {e}", meta_path.display())),
            },
            Err(e) => warnings.push(format!("{}: unreadable: {e}", meta_path.display())),
        }
    }
    let mut ids = std::collections::HashSet::new();
    found.retain(|model| {
        if ids.insert(model.id.clone()) {
            true
        } else {
            warnings.push(format!(
                "{}: duplicate model id '{}'; assign a distinct id in metadata.json",
                model.gguf_path().display(),
                model.id
            ));
            false
        }
    });
    found.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    (found, warnings)
}

fn checked_header(root: &Path, file: &Path, warnings: &mut Vec<String>) -> Option<GgufHeader> {
    let result = (|| {
        let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
        let canonical_file = file.canonicalize().map_err(|e| e.to_string())?;
        if !canonical_file.starts_with(canonical_root) {
            return Err("file resolves outside the models folder".into());
        }
        if !complete_model_file(file)? {
            return Ok(None);
        }
        gguf_header(file).map(Some)
    })();
    match result {
        Ok(header) => header,
        Err(error) => {
            warnings.push(format!("{}: {error}", file.display()));
            None
        }
    }
}

/// Small, structurally valid GGUF metadata fixture; not executable model weights.
#[cfg(test)]
pub(crate) fn test_gguf_bytes() -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    let key = b"general.architecture";
    bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&5u64.to_le_bytes());
    bytes.extend_from_slice(b"llama");
    let key = b"llama.context_length";
    bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&8192u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(b"x");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.resize(bytes.len().div_ceil(32) * 32 + 4, 0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> ModelMetadata {
        ModelMetadata {
            id: id.into(),
            name: id.into(),
            architecture: "llama".into(),
            quantization: "Q4_K_M".into(),
            chat_template: Default::default(),
            load_issues: Vec::new(),
            builtin_chat_format: None,
            tool_support_source: None,
            template_fingerprint: None,
            tooling: None,
            tooling_state: None,
            parameters: "8B".into(),
            context_length: 32768,
            vision: false,
            tool_calling: true,
            supports_reasoning: false,
            kv_bytes_per_token: None,
            weights_bytes: None,
            block_count: None,
            draft_head_layers: None,
            projector_file: None,
            model_file: None,
            dir: PathBuf::from(format!("models/{id}")),
            capabilities: ModelCapabilities {
                chat: true,
                coding: true,
                tool_calling: true,
                json_output: true,
                vision: false,
                audio: false,
            },
            loaded: false,
        }
    }

    #[test]
    fn register_validates_empty_id() {
        let mut mm = ModelManager::new();
        let mut bad = sample("ok");
        bad.id = "".into();
        assert!(mm.register(bad).is_err());
    }

    #[test]
    fn switch_unloads_previous() {
        let mut mm = ModelManager::new();
        mm.register(sample("a")).unwrap();
        mm.register(sample("b")).unwrap();
        mm.switch_to("a").unwrap();
        mm.switch_to("b").unwrap();
        assert!(!mm.get("a").unwrap().loaded);
        assert!(mm.get("b").unwrap().loaded);
        assert_eq!(mm.current().unwrap().id, "b");
    }

    #[test]
    fn auto_config_full_gpu_when_fits() {
        let c = auto_configure(8.7, 12.0, 64.0, 32768);
        // 8.7 + 16.4 KV > 12, so this particular case is hybrid, not full.
        assert!(c.gpu_layers_percent < 100);
        let small = auto_configure(4.0, 12.0, 64.0, 4096);
        assert_eq!(small.gpu_layers_percent, 100);
    }

    #[test]
    fn scan_discovers_metadata_and_skips_bad() {
        let root = std::env::temp_dir().join(format!("companion-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("good")).unwrap();
        std::fs::create_dir_all(root.join("bad")).unwrap();
        std::fs::write(
            root.join("good").join("metadata.json"),
            r#"{"id":"m1","name":"M1","architecture":"llama","quantization":"Q4_K_M","parameters":"8B","context_length":8192,"vision":false,"tool_calling":true}"#,
        )
        .unwrap();
        std::fs::write(root.join("good").join("model.gguf"), test_gguf_bytes()).unwrap();
        std::fs::write(root.join("bad").join("metadata.json"), "{not json").unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "m1");
        assert!(found[0].dir.ends_with("good"));
        assert_eq!(warnings.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_discovers_bare_named_gguf() {
        let root = std::env::temp_dir().join(format!("companion-bare-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("example-12b");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("example-12b-it-Q4_K_M.gguf"), test_gguf_bytes()).unwrap();

        let (found, warnings) = scan_models_dir(&root);
        assert!(warnings.is_empty());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "example-12b");
        assert_eq!(found[0].parameters, "12B");
        assert_eq!(found[0].quantization, "Q4_K_M");
        // The header, not the marketing filename, supplies runtime facts.
        assert_eq!(found[0].architecture, "llama");
        assert_eq!(found[0].context_length, 8192);
        assert_eq!(
            found[0].gguf_path(),
            folder.join("example-12b-it-Q4_K_M.gguf")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn scan_fixture() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("companion-model-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn scan_rejects_partial_downloads_and_refreshes_changed_headers() {
        let root = scan_fixture();
        let file = root.join("new-model.gguf");
        std::fs::write(&file, b"GGUF").unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert!(found.is_empty());
        assert_eq!(warnings.len(), 1);
        std::fs::write(&file, test_gguf_bytes()).unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert_eq!(found.len(), 1);
        assert!(warnings.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_exposes_all_variants_without_claiming_reasoning() {
        let root = scan_fixture();
        let folder = root.join("variants");
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["QwQ-8B-Q4_K_M.gguf", "QwQ-8B-Q8_0.gguf", "mmproj.gguf"] {
            std::fs::write(folder.join(name), test_gguf_bytes()).unwrap();
        }
        let (found, warnings) = scan_models_dir(&root);
        assert!(warnings.is_empty());
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].id, found[1].id);
        assert!(found
            .iter()
            .all(|model| !model.supports_reasoning && !model.tool_calling));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_missing_model_file_never_loads_a_different_weight_file() {
        let root = scan_fixture();
        let folder = root.join("explicit");
        std::fs::create_dir_all(&folder).unwrap();
        let mut model = sample("explicit");
        model.model_file = Some("selected.gguf".into());
        std::fs::write(
            folder.join("metadata.json"),
            serde_json::to_vec(&model).unwrap(),
        )
        .unwrap();
        std::fs::write(folder.join("different.gguf"), test_gguf_bytes()).unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert!(found.is_empty());
        assert_eq!(warnings.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn split_models_require_every_shard_and_register_only_first() {
        let root = scan_fixture();
        std::fs::write(root.join("large-00001-of-00002.gguf"), test_gguf_bytes()).unwrap();
        assert!(scan_models_dir(&root).0.is_empty());
        std::fs::write(root.join("large-00002-of-00002.gguf"), test_gguf_bytes()).unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert_eq!(found.len(), 1);
        assert!(warnings.is_empty());
        assert!(found[0].gguf_path().ends_with("large-00001-of-00002.gguf"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scanner_rejects_duplicate_ids_deterministically() {
        let root = scan_fixture();
        for name in ["one", "two"] {
            let folder = root.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join("metadata.json"),
                serde_json::to_vec(&sample("duplicate")).unwrap(),
            )
            .unwrap();
            std::fs::write(folder.join("model.gguf"), test_gguf_bytes()).unwrap();
        }
        let (found, warnings) = scan_models_dir(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(found[0].dir.ends_with("one"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_cannot_traverse_projector_or_model_paths() {
        for file in ["../outside.gguf", "C:\\outside.gguf", "nested/model.gguf"] {
            let mut model = sample("safe");
            model.projector_file = Some(file.into());
            assert!(model.validate().is_err());
            model.projector_file = None;
            model.model_file = Some(file.into());
            assert!(model.validate().is_err());
        }
    }

    #[test]
    fn metadata_loaded_flag_cannot_fabricate_a_running_model() {
        let mut manager = ModelManager::new();
        let mut model = sample("safe");
        model.loaded = true;
        manager.register(model).unwrap();
        assert!(!manager.get("safe").unwrap().loaded);
        manager.switch_to("safe").unwrap();
        manager.register(sample("safe")).unwrap();
        assert!(manager.get("safe").unwrap().loaded);
        let mut replacement = sample("safe");
        replacement.model_file = Some("different.gguf".into());
        assert!(manager.register(replacement).is_err());
    }

    #[test]
    fn root_level_model_deletion_resolves_only_its_weight_file() {
        let root = scan_fixture();
        std::fs::write(root.join("bare.gguf"), test_gguf_bytes()).unwrap();
        let model = scan_models_dir(&root).0.pop().unwrap();
        assert_eq!(
            model.deletion_path(&root).unwrap(),
            root.join("bare.gguf").canonicalize().unwrap()
        );
        let mut outside = model.clone();
        outside.dir = root.parent().unwrap().to_path_buf();
        assert!(outside.deletion_path(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_names_are_not_quantization_evidence() {
        assert_eq!(infer_quantization("Example-8B"), "unknown");
        assert_eq!(infer_quantization("Example-8B-Q4_K_M"), "Q4_K_M");
        assert_eq!(infer_quantization("Example-BF16"), "BF16");
    }

    #[test]
    fn chat_template_is_the_evidence_for_thinking_and_tool_support() {
        let thinking_template = "{%- if enable_thinking is defined and enable_thinking is false %}<think>\n\n</think>{%- endif %}{%- for tool in tools %}{{ tool | tojson }}{%- endfor %}";
        assert_eq!(
            template_hints(thinking_template),
            TemplateHints {
                thinking: true,
                tools: true,
                fingerprint: Some(crate::tooling::template_fingerprint(thinking_template)),
                shape: Default::default(),
            }
        );
        let chatml = "{% for message in messages %}<|im_start|>{{ message.role }}\n{{ message.content }}<|im_end|>\n{% endfor %}";
        assert_eq!(
            template_hints(chatml),
            TemplateHints { fingerprint: Some(crate::tooling::template_fingerprint(chatml)), ..TemplateHints::default() }
        );
        // A file whose template declares thinking is registered as reasoning-
        // capable even without metadata.json; a plain template is not.
        let root = scan_fixture();
        let folder = root.join("thinker");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("thinker-8b-q4_k_m.gguf"), gguf_with_template(thinking_template)).unwrap();
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("plain-8b-q4_k_m.gguf"), test_gguf_bytes()).unwrap();
        let (found, warnings) = scan_models_dir(&root);
        assert!(warnings.is_empty(), "{warnings:?}");
        let thinker = found.iter().find(|m| m.id == "thinker").unwrap();
        assert!(thinker.supports_reasoning && thinker.tool_calling);
        let plain = found.iter().find(|m| m.id == "plain").unwrap();
        assert!(!plain.supports_reasoning && !plain.tool_calling);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_a_template_that_renders_tools_declares_tool_support() {
        assert!(template_handles_tools("{%- if tools %}{{ tools | tojson }}{% endif %}"));
        assert!(template_handles_tools("{% for tool in tools %}"));
        assert!(template_handles_tools("{{ message.tool_calls }}"));
        assert!(!template_handles_tools("you are a helpful assistant with many tools. {{ messages }}"));
        assert!(!template_handles_tools("{% if tools_disabled %}{% endif %}"));
    }

    #[test]
    fn a_model_whose_files_were_deleted_leaves_the_list() {
        let root = std::env::temp_dir().join(format!("companion-reconcile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let entry = |id: &str| {
            let mut model = sample(id);
            model.dir = root.join(id);
            model.model_file = Some("model.gguf".into());
            model
        };
        for id in ["kept", "deleted", "loaded"] {
            std::fs::create_dir_all(root.join(id)).unwrap();
            std::fs::write(root.join(id).join("model.gguf"), b"x").unwrap();
        }
        let mut manager = ModelManager::new();
        let (removed, _) = manager.reconcile(vec![entry("kept"), entry("deleted"), entry("loaded")]);
        assert!(removed.is_empty());
        manager.switch_to("loaded").unwrap();
        // The user deletes two folders by hand; the next scan finds only one model.
        std::fs::remove_dir_all(root.join("deleted")).unwrap();
        std::fs::remove_dir_all(root.join("loaded")).unwrap();
        let (removed, _) = manager.reconcile(vec![entry("kept")]);
        assert_eq!(removed, vec!["deleted".to_string()]);
        let ids: Vec<String> = manager.list().into_iter().map(|model| model.id).collect();
        // The loaded model stays until it is unloaded; the deleted one is gone.
        assert_eq!(ids, vec!["kept".to_string(), "loaded".to_string()]);
        // An emptied folder is reconciled too, not skipped.
        std::fs::remove_dir_all(root.join("kept")).unwrap();
        manager.remove("loaded");
        let (removed, _) = manager.reconcile(Vec::new());
        assert_eq!(removed, vec!["kept".to_string()]);
        assert!(manager.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn structural_load_issues_are_reported_without_naming_models() {
        let clean = GgufLayout {
            tower_tensors: 0,
            vocab_tokens: Some(256_000),
            embedding_rows: Some(256_000),
            has_chat_template: true,
        };
        assert!(load_issues(clean, None).is_empty());
        // The layout of a registry blob: towers packed in, tokenizer one entry
        // longer than the embedding, no template.
        let packed = GgufLayout {
            tower_tensors: 439,
            vocab_tokens: Some(262_145),
            embedding_rows: Some(262_144),
            has_chat_template: false,
        };
        let issues = load_issues(packed, None);
        assert_eq!(issues.len(), 3);
        assert!(issues[0].contains("439 vision/audio tensors"));
        assert!(issues[1].contains("262145") && issues[1].contains("262144"));
        assert!(issues[2].contains("no chat template"));
    }

    #[test]
    fn a_file_without_a_template_uses_its_architectures_built_in_format() {
        let no_template = GgufLayout { has_chat_template: false, ..GgufLayout::default() };
        // An architecture with one format: no load issue, the format is used.
        assert!(load_issues(no_template, Some("gemma3")).is_empty());
        let header = GgufHeader {
            architecture: Some("gemma3".into()),
            layout: no_template,
            ..GgufHeader::default()
        };
        assert_eq!(fallback_chat_format(&header).as_deref(), Some("gemma"));
        // Its own template always wins.
        let templated = GgufHeader { layout: GgufLayout { has_chat_template: true, ..no_template }, ..header };
        assert_eq!(fallback_chat_format(&templated), None);
        // An architecture shared by releases with different formats keeps the
        // issue and gets no guess.
        assert_eq!(builtin_chat_format("llama"), None);
        assert_eq!(load_issues(no_template, Some("llama")).len(), 1);
        assert_eq!(load_issues(no_template, None).len(), 1);
        // The runtime's own fallback is ChatML, which some architectures use.
        assert_eq!(builtin_chat_format("qwen3moe"), Some("chatml"));
    }

    #[test]
    fn a_templates_own_refusals_describe_its_shape() {
        // Some templates reject the message list itself rather than rendering
        // it, and llama-server returns 400 before generating a token. The
        // refusal text is the only evidence, and it is read from whatever
        // template the GGUF carries -- no template is known to this code.
        let refuses = "{% if messages[0]['role'] == 'system' %}\
            {{ raise_exception('System role not supported') }}{% endif %}\
            {% for message in messages %}{% if (message['role'] == 'user') != (loop.index0 % 2 == 0) %}\
            {{ raise_exception('Conversation roles must alternate user/assistant/user/assistant/...') }}\
            {% endif %}{{ message['content'] }}{% endfor %}";
        assert_eq!(
            template_hints(refuses).shape,
            crate::inference::ChatTemplateShape {
                system_role: false,
                strict_alternation: true,
            }
        );
        // A template that raises about neither keeps every freedom, and so
        // does one that never raises at all.
        let plain = "{% for message in messages %}{{ message['role'] }}{% endfor %}";
        assert_eq!(
            template_hints(plain).shape,
            crate::inference::ChatTemplateShape::default()
        );
        let unrelated = "{% if not tools %}{{ raise_exception('Tools are required') }}{% endif %}";
        assert_eq!(
            template_hints(unrelated).shape,
            crate::inference::ChatTemplateShape::default()
        );
    }

    #[test]
    fn a_split_model_counts_every_shard() {
        let dir = std::env::temp_dir().join(format!("companion-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("model-00001-of-00002.gguf");
        std::fs::write(&first, vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("model-00002-of-00002.gguf"), vec![0u8; 250]).unwrap();
        let single = dir.join("single.gguf");
        std::fs::write(&single, vec![0u8; 40]).unwrap();
        assert_eq!(model_set_bytes(&first), 350);
        assert_eq!(model_set_bytes(&single), 40);
        assert_eq!(model_set_bytes(&dir.join("missing.gguf")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kv_bytes_per_token_follow_the_attention_shape() {
        // An 8B model: 36 layers, 8 KV heads, head dim 128 -> 147,456 bytes/token at f16.
        let eight_b = AttentionShape {
            block_count: Some(36),
            head_count: Some(32),
            head_count_kv: Some(8),
            embedding_length: Some(4096),
            key_length: Some(128),
            value_length: Some(128),
        };
        assert_eq!(eight_b.kv_bytes_per_token(), Some(147_456));
        // Without explicit key/value lengths the head dimension is derived.
        let derived = AttentionShape {
            block_count: Some(48),
            head_count: Some(40),
            head_count_kv: Some(8),
            embedding_length: Some(5120),
            ..AttentionShape::default()
        };
        assert_eq!(derived.kv_bytes_per_token(), Some(48 * 8 * 256 * 2));
        assert_eq!(AttentionShape::default().kv_bytes_per_token(), None);
    }

    #[test]
    fn a_single_projector_next_to_the_weights_enables_vision() {
        let root = scan_fixture();
        let folder = root.join("seeing");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("seeing-4b-q4_k_m.gguf"), test_gguf_bytes()).unwrap();
        std::fs::write(folder.join("mmproj-seeing-f16.gguf"), test_gguf_bytes()).unwrap();
        let (found, _) = scan_models_dir(&root);
        let model = found.iter().find(|m| m.id == "seeing").unwrap();
        assert!(model.vision);
        assert_eq!(model.projector_file.as_deref(), Some("mmproj-seeing-f16.gguf"));
        // Two projectors are ambiguous: leave vision to explicit metadata.
        std::fs::write(folder.join("mmproj-seeing-bf16.gguf"), test_gguf_bytes()).unwrap();
        let (found, _) = scan_models_dir(&root);
        assert!(!found.iter().find(|m| m.id == "seeing").unwrap().vision);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Same fixture as `test_gguf_bytes` with one extra string key.
    fn gguf_with_template(template: &str) -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&3u64.to_le_bytes());
        for (key, value) in [
            ("general.architecture", "llama"),
            ("tokenizer.chat_template", template),
        ] {
            bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(&8u32.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        let key = b"llama.context_length";
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&8192u32.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(b"x");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.resize(bytes.len().div_ceil(32) * 32 + 4, 0);
        bytes
    }

    #[test]
    #[ignore = "read-only diagnostic for locally installed model files"]
    fn inspect_installed_models() {
        let config = crate::config::AppConfig::from_env();
        let first = std::time::Instant::now();
        let (found, warnings) = scan_models_dir(&config.models_dir);
        println!("{} model(s), first scan {:?}", found.len(), first.elapsed());
        for model in &found {
            println!(
                "{}: architecture={}, context={}, file={}",
                model.id,
                model.architecture,
                model.context_length,
                model.gguf_path().display()
            );
        }
        for warning in &warnings {
            println!("warning: {warning}");
        }
        let second = std::time::Instant::now();
        let rescanned = scan_models_dir(&config.models_dir);
        println!("cached scan {:?}", second.elapsed());
        assert_eq!(found.len(), rescanned.0.len());
        assert!(!found.is_empty(), "no local GGUF files discovered");
    }
}
