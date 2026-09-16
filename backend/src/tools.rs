//! Tool system (§23–§24, §60): schemas, risk levels, validated handlers.
//!
//! Pipeline: LLM -> structured ToolRequest -> registry lookup -> workspace +
//! permission validation -> handler -> ToolResult -> model (§92).

use crate::permissions::RiskLevel;
use crate::workspace::{WorkspaceError, WorkspaceManager};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: RiskLevel,
    pub permission_required: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub name: String,
    pub args: serde_json::Value,
    /// Set once the user approves in the permission UX (§26).
    #[serde(default)]
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

impl ToolResult {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            output,
            exit_code: None,
        }
    }
    pub fn fail(output: String) -> Self {
        Self {
            ok: false,
            output,
            exit_code: None,
        }
    }
}

#[derive(Debug)]
pub enum ToolError {
    UnknownTool(String),
    InvalidArgs(String),
    PermissionRequired { tool: String, reason: String },
    Workspace(WorkspaceError),
    Io(std::io::Error),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool(t) => write!(f, "Unknown tool '{t}'."),
            Self::InvalidArgs(m) => write!(f, "Invalid tool arguments: {m}"),
            Self::PermissionRequired { tool, reason } => {
                write!(
                    f,
                    "Permission required for '{tool}': {reason} [Allow Once] [Deny]"
                )
            }
            Self::Workspace(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "Tool I/O error: {e}"),
        }
    }
}

pub fn registry() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "list_directory",
            description: "List files in a workspace directory",
            risk: RiskLevel::Safe,
            permission_required: "workspace read",
        },
        ToolDescriptor {
            name: "read_file",
            description: "Read a numbered text chunk: path, optional 1-based start_line/end_line (200 lines default, 500 max). Any line is accessible; follow the returned continuation for more. Optional start_column continues an unusually long line.",
            risk: RiskLevel::Safe,
            permission_required: "workspace read",
        },
        ToolDescriptor {
            name: "write_file",
            description: "Create or overwrite a workspace file",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "append_file",
            description: "Add text to the end of a workspace file, creating it if absent",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "edit_file",
            description: "Replace exact existing text: old (copied from read_file, without the line-number labels) becomes new. old must occur once; add surrounding lines to make it unique. A unified-diff patch argument is also accepted.",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "delete_file",
            description: "Delete a workspace file",
            risk: RiskLevel::Dangerous,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "search_text",
            description: "Regex search in a workspace directory or individual file. Returns matching line numbers; use read_file start_line/end_line for surrounding code. Narrow path/query when results are capped.",
            risk: RiskLevel::Safe,
            permission_required: "workspace read",
        },
        ToolDescriptor {
            name: "execute_command",
            description: "Run a shell command with captured output",
            risk: RiskLevel::Dangerous,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "web_search",
            description: "Search the web (explicit opt-in per request)",
            risk: RiskLevel::Moderate,
            permission_required: "explicit (Search toggle)",
        },
        ToolDescriptor {
            name: "create_document",
            description: "Generate txt/md/json/csv/html/xlsx/docx/pdf/pptx from a JSON spec",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "system_info",
            description: "OS/CPU/RAM/GPU summary",
            risk: RiskLevel::Safe,
            permission_required: "none",
        },
        ToolDescriptor {
            name: "git_commit",
            description: "Commit staged workspace changes with a message",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
        ToolDescriptor {
            name: "list_processes",
            description: "List running OS processes (name + pid)",
            risk: RiskLevel::Safe,
            permission_required: "none",
        },
        ToolDescriptor {
            name: "open_path",
            description: "Open a workspace file/folder with the OS default app",
            risk: RiskLevel::Moderate,
            permission_required: "explicit",
        },
    ]
}

pub fn risk_of(name: &str) -> RiskLevel {
    match name {
        "execute_command" | "delete_file" => RiskLevel::Dangerous,
        "write_file" | "append_file" | "edit_file" | "web_search" | "create_document"
        | "git_commit" | "open_path" => RiskLevel::Moderate,
        _ => RiskLevel::Safe,
    }
}

/// Tool names the in-chat loop may run WITHOUT asking (Stage 31): read-only,
/// workspace-confined, no side effects. Everything else needs the agent
/// panel/commands approval flow.
/// Tools a chat reply may run without approval: read-only inspection, plus
/// `create_document`, which only ever writes into the app's own artifacts
/// folder (never the project) and hands the user a file to open or save.
pub fn chat_safe(name: &str) -> bool {
    matches!(
        name,
        "list_directory"
            | "read_file"
            | "search_text"
            | "system_info"
            | "list_processes"
            | "create_document"
    )
}

const MAX_FILE_BYTES: u64 = 5_000_000;

/// Marks a write the host read back and found identical to what was sent.
pub const WRITE_VERIFIED: &str = "(verified on disk)";
const MAX_SEARCH_MATCHES: usize = 50;

fn require_approved(req: &ToolRequest, approved: bool, what: &str) -> Result<(), ToolError> {
    if approved || req.approved {
        return Ok(());
    }
    Err(ToolError::PermissionRequired {
        tool: req.name.clone(),
        reason: what.into(),
    })
}

/// Execute a validated tool. SAFE reads run immediately; MODERATE/DANGEROUS
/// require `req.approved == true` (set by the permission UX, §26).
pub fn execute(
    req: &ToolRequest,
    ws: &WorkspaceManager,
    approved: bool,
) -> Result<ToolResult, ToolError> {
    match req.name.as_str() {
        "list_directory" => {
            let rel = req.args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let dir = ws.resolve(rel).map_err(ToolError::Workspace)?;
            let entries = std::fs::read_dir(&dir).map_err(ToolError::Io)?;
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            Ok(ToolResult::ok(names.join("\n")))
        }
        "read_file" => {
            let rel = req.args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("read_file requires {\"path\": \"...\"}".into())
            })?;
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            crate::file_read::read(&p, &req.args)
        }
        "write_file" => {
            require_approved(req, approved, "File creation needs explicit approval.")?;
            let rel = req.args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("write_file requires {\"path\": \"...\", \"content\": \"...\"}".into())
            })?;
            let content = req.args.get("content").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("write_file requires {\"path\": \"...\", \"content\": \"...\"}".into())
            })?;
            if content.len() as u64 > MAX_FILE_BYTES {
                return Err(ToolError::InvalidArgs(format!(
                    "content too large ({} bytes, max {MAX_FILE_BYTES})", content.len()
                )));
            }
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(ToolError::Io)?;
            }
            std::fs::write(&p, content).map_err(ToolError::Io)?;
            // Read the file back: the host can establish byte for byte that
            // the requested content is what is on disk, which is stronger
            // evidence than asking the model to read its own text again (and
            // costs no context on a small window).
            let on_disk = std::fs::read(&p).map_err(ToolError::Io)?;
            if on_disk != content.as_bytes() {
                return Err(ToolError::Io(std::io::Error::other(format!(
                    "{rel} was written but reads back as {} bytes instead of the {} sent; check the path and disk space",
                    on_disk.len(),
                    content.len()
                ))));
            }
            Ok(ToolResult::ok(format!(
                "wrote {} bytes ({} lines) to {rel} {WRITE_VERIFIED}",
                content.len(),
                content.lines().count()
            )))
        }
        // Writing a long file in parts is the only way to produce one at all
        // when a reply cannot hold it: a 5K-token window leaves room for a few
        // thousand characters per step. Appending also costs no context for
        // an anchor, which edit_file needs and can fail to match.
        "append_file" => {
            require_approved(req, approved, "File modifications need explicit approval.")?;
            const USAGE: &str =
                "append_file requires {\"path\": \"...\", \"content\": \"text to add at the end\"}";
            let rel = req
                .args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArgs(USAGE.into()))?;
            let content = req
                .args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArgs(USAGE.into()))?;
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            let existing = match std::fs::metadata(&p) {
                Ok(meta) => meta.len(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                Err(e) => return Err(ToolError::Io(e)),
            };
            if existing + content.len() as u64 > MAX_FILE_BYTES {
                return Err(ToolError::InvalidArgs(format!(
                    "file would exceed {MAX_FILE_BYTES} bytes ({existing} already written)"
                )));
            }
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(ToolError::Io)?;
            }
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)
                .map_err(ToolError::Io)?;
            file.write_all(content.as_bytes()).map_err(ToolError::Io)?;
            file.flush().map_err(ToolError::Io)?;
            drop(file);
            // Same read-back as write_file: the host confirms the bytes landed
            // rather than asking the model to take its own word for it.
            let on_disk = std::fs::metadata(&p).map(|meta| meta.len()).unwrap_or(0);
            let expected = existing + content.len() as u64;
            if on_disk != expected {
                return Err(ToolError::Io(std::io::Error::other(format!(
                    "{rel} reads back as {on_disk} bytes instead of the expected {expected}"
                ))));
            }
            Ok(ToolResult::ok(format!(
                "appended {} bytes ({} lines) to {rel}; it is now {on_disk} bytes {WRITE_VERIFIED}",
                content.len(),
                content.lines().count()
            )))
        }
        "edit_file" => {
            require_approved(req, approved, "File modifications need explicit approval.")?;
            const USAGE: &str = "edit_file requires {\"path\": \"...\", \"old\": \"exact existing text\", \"new\": \"replacement text\"} (or {\"path\", \"patch\"} with a unified diff)";
            let rel = req.args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs(USAGE.into())
            })?;
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            let original = std::fs::read_to_string(&p).map_err(ToolError::Io)?;
            if original.len() as u64 > MAX_FILE_BYTES {
                return Err(ToolError::InvalidArgs("file too large to patch; rewrite it in chunks".into()));
            }
            // Exact-text replacement is what small local models produce
            // reliably: they copy the lines they saw and write the new ones.
            // Unified diffs stay supported for models that emit them well.
            let updated = if let Some(old) = req.args.get("old").and_then(|v| v.as_str()) {
                let new = req.args.get("new").and_then(|v| v.as_str()).ok_or_else(|| {
                    ToolError::InvalidArgs(USAGE.into())
                })?;
                let replace_all = req.args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
                apply_replacement(&original, old, new, replace_all).map_err(ToolError::InvalidArgs)?
            } else if let Some(patch) = req.args.get("patch").and_then(|v| v.as_str()) {
                apply_unified_patch(&original, patch).map_err(ToolError::InvalidArgs)?
            } else {
                return Err(ToolError::InvalidArgs(USAGE.into()));
            };
            std::fs::write(&p, &updated).map_err(ToolError::Io)?;
            Ok(ToolResult::ok(format!(
                "patched {rel}: {} -> {} bytes",
                original.len(),
                updated.len()
            )))
        }
        "delete_file" => {
            require_approved(req, approved, "File deletion needs explicit approval.")?;
            let rel = req.args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("delete_file requires {\"path\": \"...\"}".into())
            })?;
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            if !p.is_file() {
                return Err(ToolError::InvalidArgs(format!("not a file: {rel}")));
            }
            std::fs::remove_file(&p).map_err(ToolError::Io)?;
            Ok(ToolResult::ok(format!("deleted {rel}")))
        }
        "search_text" => {
            let q = req.args.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("search_text requires {\"query\": \"...\"}".into())
            })?;
            let rel = req.args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let dir = ws.resolve(rel).map_err(ToolError::Workspace)?;
            let re = regex::Regex::new(q)
                .map_err(|e| ToolError::InvalidArgs(format!("bad regex: {e}")))?;
            Ok(ToolResult::ok(search_dir(&dir, &re)))
        }
        "execute_command" => {
            require_approved(req, approved, "Command execution needs explicit approval.")?;
            let cmd = req.args.get("command").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("execute_command requires {\"command\": \"...\"}".into())
            })?;
            let timeout = req.args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(crate::terminal::DEFAULT_TIMEOUT_SECS);
            let cwd = ws.resolve(req.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("."))
                .map_err(ToolError::Workspace)?;
            if !cwd.is_dir() {
                return Err(ToolError::InvalidArgs("cwd is not a directory".into()));
            }
            match crate::terminal::run(cmd, &cwd, timeout) {
                Ok(r) => {
                    let mut res = ToolResult::ok(crate::terminal::format_result(cmd, &r));
                    res.exit_code = r.exit_code;
                    Ok(res)
                }
                Err(e) => Err(ToolError::InvalidArgs(e)),
            }
        }
        "system_info" => Ok(ToolResult::ok(format!(
            "os={} arch={}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))),
        // Stage 36: version control with guardrails. Reads run through the
        // shell allowlist; commit is MODERATE (approval). Destructive git
        // (reset --hard, push --force, clean -fd) is refused outright — use
        // the Git panel's explicit confirm flow instead.
        "git_commit" => {
            require_approved(req, approved, "Committing needs explicit approval.")?;
            let msg = req.args.get("message").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("git_commit requires {\"message\": \"...\"}".into())
            })?;
            if msg.trim().is_empty() || msg.len() > 500 {
                return Err(ToolError::InvalidArgs("commit message must be 1–500 chars".into()));
            }
            if msg.contains(['\n', '\r', '"', '\'', '`', '$', ';', '&', '|']) {
                return Err(ToolError::InvalidArgs("commit message must be a single plain line".into()));
            }
            let root = ws.root().to_path_buf();
            let status = std::process::Command::new("git")
                .args(["-C", &root.to_string_lossy(), "commit", "-m", msg.trim()])
                .output()
                .map_err(ToolError::Io)?;
            let body = format!("{}{}", String::from_utf8_lossy(&status.stdout), String::from_utf8_lossy(&status.stderr));
            if status.status.success() {
                Ok(ToolResult::ok(body.chars().take(2000).collect()))
            } else {
                Ok(ToolResult { ok: false, output: body.chars().take(2000).collect(), exit_code: status.status.code() })
            }
        }
        // Stage 37: automation primitives. list_processes is read-only (SAFE);
        // open_path launches the OS default handler (MODERATE, approval).
        "list_processes" => {
            let mut sys = sysinfo::System::new_all();
            sys.refresh_processes();
            let mut rows: Vec<String> = sys.processes().values()
                .map(|p| format!("{} (pid {})", p.name(), p.pid()))
                .collect();
            rows.sort();
            rows.truncate(300);
            Ok(ToolResult::ok(rows.join("\n")))
        }
        "open_path" => {
            require_approved(req, approved, "Opening apps/files needs explicit approval.")?;
            let rel = req.args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            // The OS handler is fed only paths that exist. Given a missing
            // path, Windows Explorer opens an unrelated folder window over
            // the app and the call would still have looked successful.
            if !p.exists() {
                return Err(ToolError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "nothing exists at {rel} in the workspace, so nothing was opened. open_path opens an existing file or folder of the project; documents made with create_document live in the chat's Artifacts panel, where the user opens them."
                    ),
                )));
            }
            let target = p.to_string_lossy().into_owned();
            let mut cmd = match std::env::consts::OS {
                "windows" => {
                    let mut c = std::process::Command::new("explorer");
                    c.arg(&target);
                    c
                }
                "macos" => {
                    let mut c = std::process::Command::new("open");
                    c.arg(&target);
                    c
                }
                _ => {
                    let mut c = std::process::Command::new("xdg-open");
                    c.arg(&target);
                    c
                }
            };
            match cmd.spawn() {
                Ok(_) => Ok(ToolResult::ok(format!("opened {rel} with the OS default handler"))),
                Err(e) => Err(ToolError::Io(e)),
            }
        }
        // Stage 11: web search needs async HTTP + provider config, so it runs
        // in the async API layer (chat handler / agent loop / tools endpoint),
        // never here. The registry entry above documents it for the model.
        "web_search" => Err(ToolError::InvalidArgs(
            "web_search executes in the async request path with explicit Search consent, not via sync execute()".into(),
        )),
        other => Err(ToolError::UnknownTool(other.into())),
    }
}

/// Apply a unified diff to `original` (§31: structured patches, not rewrites).
/// Supports multiple `@@` hunks with context verification (fuzz 0). The
/// `---`/`+++` headers are informational; the caller already resolved the
/// target path through the workspace manager.
pub fn apply_unified_patch(original: &str, patch: &str) -> Result<String, String> {
    // Hunk headers from local models are routinely wrong about line numbers
    // and counts, while the context and removed lines are usually right. Each
    // hunk is therefore located by its old-side lines (exact first, then
    // ignoring surrounding whitespace), starting from the header's hint and
    // scanning forward from the previous hunk. Counts in the header are not
    // trusted; the located lines are.
    struct Hunk {
        hint: usize,
        lines: Vec<(char, String)>,
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("---") || line.starts_with("+++") || line.starts_with("diff ") {
            continue;
        }
        if line.starts_with("@@") {
            let (start, _) = parse_hunk_header(line)?;
            hunks.push(Hunk {
                hint: start.saturating_sub(1),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            return Err(format!("expected '@@' hunk header, got: {line}"));
        };
        let (kind, text) = if line.is_empty() {
            (' ', "")
        } else {
            let mut chars = line.chars();
            (chars.next().unwrap_or(' '), chars.as_str())
        };
        match kind {
            ' ' | '-' | '+' => hunk.lines.push((kind, text.to_string())),
            '\\' => {} // "\ No newline at end of file"
            _ => {
                return Err(format!(
                    "bad hunk line: {line:?} (must start with ' ', '-', '+')"
                ))
            }
        }
    }
    if hunks.is_empty() {
        return Err("patch contains no hunks".into());
    }
    let orig: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    for hunk in &hunks {
        let old: Vec<&str> = hunk
            .lines
            .iter()
            .filter(|(kind, _)| *kind != '+')
            .map(|(_, text)| text.as_str())
            .collect();
        let at = if old.is_empty() {
            // Pure insertion: the header position is all there is.
            hunk.hint.clamp(cursor, orig.len())
        } else {
            locate_lines(&orig, &old, cursor, hunk.hint).ok_or_else(|| {
                format!(
                    "the hunk's original lines were not found in the file (first expected line: {:?}). Re-read the file and use edit_file with old/new: old = the exact existing lines, new = their replacement.",
                    old[0]
                )
            })?
        };
        out.extend(orig[cursor..at].iter().map(|line| line.to_string()));
        let mut position = at;
        for (kind, text) in &hunk.lines {
            match kind {
                ' ' => {
                    // Keep the file's own line (whitespace may differ from the hunk).
                    out.push(orig.get(position).map(|l| l.to_string()).unwrap_or_else(|| text.clone()));
                    position += 1;
                }
                '-' => position += 1,
                _ => out.push(text.clone()),
            }
        }
        cursor = position.min(orig.len());
    }
    out.extend(orig[cursor..].iter().map(|line| line.to_string()));
    let mut result = out.join("\n");
    if original.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

/// Find `needle` (a run of lines) in `haystack` at or after `from`: exact
/// match nearest to `hint` first, then a whitespace-insensitive match.
fn locate_lines(haystack: &[&str], needle: &[&str], from: usize, hint: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len().saturating_sub(from) {
        return None;
    }
    let candidates = |equal: &dyn Fn(&str, &str) -> bool| -> Vec<usize> {
        (from..=haystack.len() - needle.len())
            .filter(|&start| {
                needle
                    .iter()
                    .enumerate()
                    .all(|(offset, line)| equal(haystack[start + offset], line))
            })
            .collect()
    };
    let nearest = |found: Vec<usize>| {
        found
            .into_iter()
            .min_by_key(|start| start.abs_diff(hint))
    };
    nearest(candidates(&|a, b| a == b))
        .or_else(|| nearest(candidates(&|a, b| a.trim() == b.trim())))
}

/// The file lines that best resemble the block the model tried to match:
/// the window whose lines share the most leading characters (after trimming)
/// with the requested lines. Shown verbatim so the model can copy them.
fn closest_region(haystack: &[&str], needle: &[&str]) -> Option<String> {
    if haystack.is_empty() || needle.is_empty() {
        return None;
    }
    let similarity = |a: &str, b: &str| {
        let (a, b) = (a.trim(), b.trim());
        let common = a
            .chars()
            .zip(b.chars())
            .take_while(|(x, y)| x == y)
            .count();
        // Reward matching content, not matching emptiness.
        if a.is_empty() || b.is_empty() {
            0
        } else {
            common * 100 / a.len().max(b.len())
        }
    };
    let window = needle.len().min(haystack.len());
    let (best_start, best_score) = (0..=haystack.len() - window)
        .map(|start| {
            let score: usize = needle
                .iter()
                .take(window)
                .enumerate()
                .map(|(offset, line)| similarity(haystack[start + offset], line))
                .sum();
            (start, score)
        })
        .max_by_key(|(start, score)| (*score, usize::MAX - *start))?;
    if best_score == 0 {
        return None;
    }
    let from = best_start.saturating_sub(1);
    let to = (best_start + window + 1).min(haystack.len());
    Some(
        haystack[from..to]
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Exact-text replacement with the fallbacks small models need: CRLF/LF
/// tolerance, a whitespace-insensitive line match when indentation was
/// reproduced imperfectly, and stripping of `N: ` line-number labels copied
/// from read_file output. The match must be unique unless `replace_all`.
pub fn apply_replacement(
    original: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old.is_empty() {
        return Err("old text is empty: give the exact existing lines to replace".into());
    }
    let crlf = original.contains("\r\n");
    let text = original.replace("\r\n", "\n");
    let old_lf = old.replace("\r\n", "\n");
    let new_lf = new.replace("\r\n", "\n");
    let finish = |updated: String| {
        if crlf {
            updated.replace('\n', "\r\n")
        } else {
            updated
        }
    };
    let count = text.matches(old_lf.as_str()).count();
    if count == 1 || (count > 1 && replace_all) {
        return Ok(finish(text.replace(old_lf.as_str(), &new_lf)));
    }
    if count > 1 {
        return Err(format!(
            "old text occurs {count} times; include more surrounding lines so it is unique, or set replace_all: true"
        ));
    }
    // Strip "12: " style labels a model may have copied from read_file output.
    let unlabelled: Vec<String> = old_lf
        .lines()
        .map(|line| {
            let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && line[digits..].starts_with(": ") {
                line[digits + 2..].to_string()
            } else if digits > 0 && line[digits..] == *":" {
                String::new()
            } else {
                line.to_string()
            }
        })
        .collect();
    let stripped = unlabelled.join("\n");
    if unlabelled.len() == old_lf.lines().count() && stripped != old_lf {
        let count = text.matches(stripped.as_str()).count();
        if count == 1 {
            return Ok(finish(text.replace(stripped.as_str(), &new_lf)));
        }
    }
    // Whitespace-insensitive line match: locate the block, replace it whole.
    let haystack: Vec<&str> = text.lines().collect();
    let needle: Vec<&str> = stripped.lines().collect();
    let starts: Vec<usize> = if needle.is_empty() || needle.len() > haystack.len() {
        Vec::new()
    } else {
        (0..=haystack.len() - needle.len())
            .filter(|&start| {
                needle
                    .iter()
                    .enumerate()
                    .all(|(offset, line)| haystack[start + offset].trim() == line.trim())
            })
            .collect()
    };
    match starts.len() {
        1 => {
            let start = starts[0];
            let mut out: Vec<String> = haystack[..start].iter().map(|l| l.to_string()).collect();
            out.extend(new_lf.lines().map(|l| l.to_string()));
            out.extend(haystack[start + needle.len()..].iter().map(|l| l.to_string()));
            let mut updated = out.join("\n");
            if text.ends_with('\n') {
                updated.push('\n');
            }
            Ok(finish(updated))
        }
        0 => Err(format!(
            "old text was not found in the file. Copy the exact existing lines (without the line-number labels) into old, then give their replacement in new.{}",
            closest_region(&haystack, &needle)
                .map(|region| format!("\nThe closest text in the file is:\n{region}\nUse those exact lines (or a unique subset of them) as old."))
                .unwrap_or_default()
        )),
        n => Err(format!(
            "old text matches {n} places when ignoring indentation; include more surrounding lines so it is unique"
        )),
    }
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize), String> {
    // Format: @@ -old_start[,old_len] +new_start[,new_len] @@ [section]
    let inner = line
        .strip_prefix("@@")
        .and_then(|s| s.split("@@").next())
        .ok_or_else(|| format!("bad hunk header: {line}"))?;
    let mut parts = inner.split_whitespace();
    let old = parts
        .next()
        .ok_or_else(|| format!("bad hunk header: {line}"))?;
    let old = old
        .strip_prefix('-')
        .ok_or_else(|| format!("bad hunk header: {line}"))?;
    let (start, len) = match old.split_once(',') {
        Some((a, b)) => (
            a.parse::<usize>()
                .map_err(|_| format!("bad hunk header: {line}"))?,
            b.parse::<usize>()
                .map_err(|_| format!("bad hunk header: {line}"))?,
        ),
        None => (
            old.parse::<usize>()
                .map_err(|_| format!("bad hunk header: {line}"))?,
            1,
        ),
    };
    Ok((start, len))
}

/// Recursive regex search (§24). Large source files are scanned line by line.
/// Returns `path:line: match` lines, capped — deterministic and terse for
/// small local models (§95: relevant sections, not dumps).
fn search_dir(dir: &std::path::Path, re: &regex::Regex) -> String {
    let mut hits: Vec<String> = vec![];
    if dir.is_file() {
        search_file(dir, dir.parent().unwrap_or(dir), re, &mut hits);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if hits.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            if hits.len() >= MAX_SEARCH_MATCHES {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(
                    name.as_str(),
                    "node_modules" | "target" | "build" | "dist" | "vendor" | "__pycache__"
                )
            {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                search_file(&path, dir, re, &mut hits);
            }
        }
    }
    if hits.is_empty() {
        "(no matches)".into()
    } else {
        let mut out = hits.join("\n");
        if hits.len() >= MAX_SEARCH_MATCHES {
            out.push_str(&format!("\n(capped at {MAX_SEARCH_MATCHES} matches)"));
        }
        out
    }
}

fn search_file(
    path: &std::path::Path,
    root: &std::path::Path,
    re: &regex::Regex,
    hits: &mut Vec<String>,
) {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = std::io::BufReader::new(file);
    // Avoid interpreting binary/model assets as code, without a source-size cap.
    match reader.fill_buf() {
        Ok(bytes) if !bytes.contains(&0) => {}
        _ => return,
    }
    for (n, line) in reader.lines().enumerate() {
        if hits.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let Ok(line) = line else {
            break;
        };
        if line.contains('\0') {
            break;
        }
        if re.is_match(&line) {
            hits.push(format!(
                "{}:{}: {}",
                path.strip_prefix(root).unwrap_or(path).display(),
                n + 1,
                line.chars().take(200).collect::<String>()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws() -> WorkspaceManager {
        // Unique per call: parallel tests must never share a scratch dir.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("companion-tool-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        WorkspaceManager::new(dir)
    }

    /// The whole path a file body travels: the model's reply, parsed as an
    /// action, executed, read back from disk. The body is the kind that broke
    /// a real run when it went through JSON escaping.
    #[test]
    fn a_raw_file_body_reaches_disk_byte_for_byte_and_appends_in_parts() {
        let w = ws();
        let first = r#"(() => {
  "use strict";
  const re = /\d+\.\d+/;
  const msg = "it's \"quoted\"";
  const tpl = `line ${a}`;
"#;
        let second = r#"  const path = "C:\Users\name";
})();
"#;
        let reply = format!(
            concat!(
                "Writing the script.\n```tool\n",
                r#"{{"name":"write_file","args":{{"path":"app.js"}}}}"#,
                "\n<<<CONTENT\n{first}CONTENT>>>\n```\n"
            ),
            first = first
        );
        let call = crate::agent_runner::parse_tool_block(&reply).expect("parsed");
        assert_eq!(call.name, "write_file");
        let request = ToolRequest {
            name: call.name.clone(),
            args: call.args.clone(),
            approved: true,
        };
        let result = execute(&request, &w, true).unwrap();
        assert!(result.output.contains(WRITE_VERIFIED), "{}", result.output);

        // The second part arrives as its own action, as a small window forces.
        let appended = format!(
            concat!(
                "```tool\n",
                r#"{{"name":"append_file","args":{{"path":"app.js"}}}}"#,
                "\n<<<CONTENT\n{second}CONTENT>>>\n```\n"
            ),
            second = second
        );
        let call = crate::agent_runner::parse_tool_block(&appended).expect("parsed append");
        let request = ToolRequest {
            name: call.name.clone(),
            args: call.args.clone(),
            approved: true,
        };
        execute(&request, &w, true).unwrap();

        let on_disk = std::fs::read_to_string(w.root().join("app.js")).unwrap();
        assert_eq!(on_disk, format!("{first}{second}"));
        // The characters that used to be lost survived: an apostrophe, an
        // escaped quote, a regex backslash, a backtick and a Windows path.
        assert!(on_disk.contains(r#"const re = /\d+\.\d+/;"#));
        assert!(on_disk.contains(r#"const msg = "it's \"quoted\"";"#));
        assert!(on_disk.contains(r#""C:\Users\name""#));
    }

    #[test]
    fn registry_marks_command_dangerous() {
        let reg = registry();
        let cmd = reg.iter().find(|t| t.name == "execute_command").unwrap();
        assert_eq!(cmd.risk, RiskLevel::Dangerous);
    }

    #[test]
    fn search_large_source_and_then_read_deep_match() {
        let w = ws();
        let text = format!(
            "{}deep_unique_marker\n",
            format!("{}\n", "x".repeat(90)).repeat(17_000)
        );
        std::fs::write(w.root().join("large.cpp"), text).unwrap();
        for path in [".", "large.cpp"] {
            let request = ToolRequest {
                name: "search_text".into(),
                args: serde_json::json!({"path":path,"query":"deep_unique_marker"}),
                approved: false,
            };
            let result = execute(&request, &w, false).unwrap();
            assert!(result
                .output
                .contains("large.cpp:17001: deep_unique_marker"));
        }
        let request = ToolRequest {
            name: "read_file".into(),
            args: serde_json::json!({"path":"large.cpp","start_line":17001}),
            approved: false,
        };
        assert!(execute(&request, &w, false)
            .unwrap()
            .output
            .contains("17001: deep_unique_marker"));
    }

    #[test]
    fn read_file_inside_workspace_works() {
        let w = ws();
        let req = ToolRequest {
            name: "read_file".into(),
            args: serde_json::json!({"path": "a.txt"}),
            approved: false,
        };
        let r = execute(&req, &w, false).unwrap();
        assert!(r.ok && r.output.contains("hello"));
    }

    #[test]
    fn read_file_blocks_traversal() {
        let w = WorkspaceManager::new(PathBuf::from("/tmp/ws-root-test"));
        let req = ToolRequest {
            name: "read_file".into(),
            args: serde_json::json!({"path": "../../etc/passwd"}),
            approved: false,
        };
        assert!(matches!(
            execute(&req, &w, false).unwrap_err(),
            ToolError::Workspace(_)
        ));
    }

    #[test]
    fn write_requires_approval() {
        let w = ws();
        let req = ToolRequest {
            name: "write_file".into(),
            args: serde_json::json!({"path": "b.txt", "content": "x"}),
            approved: false,
        };
        assert!(matches!(
            execute(&req, &w, false).unwrap_err(),
            ToolError::PermissionRequired { .. }
        ));
    }

    #[test]
    fn unknown_tool_errors() {
        let w = ws();
        let req = ToolRequest {
            name: "nuke".into(),
            args: serde_json::json!({}),
            approved: true,
        };
        assert!(matches!(
            execute(&req, &w, true).unwrap_err(),
            ToolError::UnknownTool(_)
        ));
    }

    fn wsfresh(name: &str) -> WorkspaceManager {
        let dir = std::env::temp_dir().join(format!("companion-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        WorkspaceManager::new(dir)
    }

    #[test]
    fn write_creates_nested_and_roundtrips() {
        let w = wsfresh("write");
        let req = ToolRequest {
            name: "write_file".into(),
            args: serde_json::json!({"path": "sub/b.txt", "content": "hello"}),
            approved: true,
        };
        let r = execute(&req, &w, false).unwrap();
        assert!(r.ok);
        let back = execute(
            &ToolRequest {
                name: "read_file".into(),
                args: serde_json::json!({"path": "sub/b.txt"}),
                approved: false,
            },
            &w,
            false,
        )
        .unwrap();
        assert!(back.output.contains("1: hello\n"));
        assert!(back.output.contains("EOF: no further content"));
        assert_eq!(
            std::fs::read_to_string(w.root().join("sub/b.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn patch_applies_and_rejects_stale_context() {
        let original = "line1\nline2\nline3\n";
        let patch = "@@ -1,3 +1,3 @@\n line1\n-line2\n+LINE2\n line3\n";
        assert_eq!(
            apply_unified_patch(original, patch).unwrap(),
            "line1\nLINE2\nline3\n"
        );
        let stale = "@@ -1,3 +1,3 @@\n line1\n-CHANGED\n+X\n line3\n";
        let err = apply_unified_patch(original, stale).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(apply_unified_patch(original, "no hunks here").is_err());
    }

    #[test]
    fn replacement_edit_tolerates_labels_indentation_and_line_endings() {
        let original = "function subtract(a, b) {\n  return a + b; // BUG\n}\n\nfunction add(a, b) {\n  return a + b;\n}\n";
        // Unique exact match.
        let fixed = apply_replacement(original, "  return a + b; // BUG", "  return a - b;", false).unwrap();
        assert!(fixed.contains("return a - b;") && !fixed.contains("BUG"));
        // Ambiguous without replace_all.
        let err = apply_replacement(original, "  return a + b;", "x", false).unwrap_err();
        assert!(err.contains("2 times"), "{err}");
        assert_eq!(apply_replacement(original, "  return a + b;", "  return 0;", true).unwrap().matches("return 0;").count(), 2);
        // Line-number labels copied from read_file output are stripped.
        let labelled = apply_replacement(original, "1: function subtract(a, b) {\n2:   return a + b; // BUG", "function subtract(a, b) {\n  return a - b;", false).unwrap();
        assert!(labelled.starts_with("function subtract(a, b) {\n  return a - b;\n}"));
        // Indentation reproduced imperfectly still locates the block.
        let loose = apply_replacement(original, "return a + b; // BUG", "  return a - b;", false).unwrap();
        assert!(loose.contains("  return a - b;\n}"));
        // CRLF files keep CRLF.
        let crlf = original.replace('\n', "\r\n");
        let fixed_crlf = apply_replacement(&crlf, "  return a + b; // BUG", "  return a - b;", false).unwrap();
        assert!(fixed_crlf.contains("return a - b;\r\n}"));
        assert!(!fixed_crlf.contains("\n\n") || fixed_crlf.contains("\r\n\r\n"));
        assert!(apply_replacement(original, "nothing like this", "x", false).unwrap_err().contains("not found"));
        assert!(apply_replacement(original, "", "x", false).is_err());
        // A near miss (the model dropped the trailing comment) shows the real
        // lines so the next attempt can copy them.
        let err = apply_replacement(original, "function subtract(a, b) {\n  return a + b;\n}", "x", false).unwrap_err();
        assert!(err.contains("closest text"), "{err}");
        assert!(err.contains("return a + b; // BUG"), "{err}");
    }

    #[test]
    fn unified_patch_locates_hunks_by_content_not_by_wrong_headers() {
        let original = "// header\nfunction add(a, b) {\n  return a + b;\n}\n\nfunction subtract(a, b) {\n  return a + b; // BUG\n}\n";
        // Wrong start line and counts (what an 8B model produced), right lines.
        let patch = "--- a/calc.js\n+++ b/calc.js\n@@ -1,2 +1,2 @@\n function subtract(a, b) {\n-  return a + b; // BUG\n+  return a - b;\n }\n";
        let fixed = apply_unified_patch(original, patch).unwrap();
        assert!(fixed.contains("function subtract(a, b) {\n  return a - b;\n}"));
        assert!(fixed.contains("function add(a, b) {\n  return a + b;\n}"), "the other function is untouched");
        // Lines that do not exist anywhere are still rejected.
        let bogus = "@@ -6,2 +6,2 @@\n-  return a - b; // Fixed\n+  return a * b;\n";
        assert!(apply_unified_patch(original, bogus).unwrap_err().contains("not found"));
    }

    #[test]
    fn edit_tool_end_to_end() {
        let w = wsfresh("edit");
        std::fs::write(w.root().join("c.txt"), "a\nb\nc\n").unwrap();
        let req = ToolRequest {
            name: "edit_file".into(),
            args: serde_json::json!({"path": "c.txt", "patch": "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"}),
            approved: true,
        };
        assert!(execute(&req, &w, false).unwrap().ok);
        assert_eq!(
            std::fs::read_to_string(w.root().join("c.txt")).unwrap(),
            "a\nB\nc\n"
        );
    }

    #[test]
    fn search_finds_and_caps() {
        let w = wsfresh("search");
        std::fs::write(
            w.root().join("x.rs"),
            "fn temporal() {}\n// temporal accumulation\n",
        )
        .unwrap();
        std::fs::create_dir_all(w.root().join("sub")).unwrap();
        std::fs::write(w.root().join("sub").join("y.txt"), "nothing here\n").unwrap();
        let req = ToolRequest {
            name: "search_text".into(),
            args: serde_json::json!({"query": "temporal"}),
            approved: false,
        };
        let r = execute(&req, &w, false).unwrap();
        assert!(r.output.contains("x.rs:1"), "{}", r.output);
        assert!(r.output.contains("x.rs:2"), "{}", r.output);
        let bad = ToolRequest {
            name: "search_text".into(),
            args: serde_json::json!({"query": "(unclosed"}),
            approved: false,
        };
        assert!(matches!(
            execute(&bad, &w, false).unwrap_err(),
            ToolError::InvalidArgs(_)
        ));
    }

    #[test]
    fn writes_are_read_back_by_the_host_and_missing_paths_are_not_opened() {
        let w = wsfresh("verify");
        let req = ToolRequest {
            name: "write_file".into(),
            args: serde_json::json!({"path": "site/index.html", "content": "<h1>Hi</h1>\n<p>Energy</p>\n"}),
            approved: true,
        };
        let result = execute(&req, &w, false).unwrap();
        assert!(result.output.contains(WRITE_VERIFIED), "{}", result.output);
        assert!(result.output.contains("(2 lines)"), "{}", result.output);
        assert_eq!(std::fs::read_to_string(w.root().join("site/index.html")).unwrap(), "<h1>Hi</h1>\n<p>Energy</p>\n");
        // open_path never hands the OS a path that does not exist (Explorer
        // would open an unrelated window and the call would look successful).
        let open = ToolRequest {
            name: "open_path".into(),
            args: serde_json::json!({"path": "energy-drink.txt"}),
            approved: true,
        };
        let error = execute(&open, &w, false).unwrap_err().to_string();
        assert!(error.contains("nothing exists at energy-drink.txt"), "{error}");
        assert!(error.contains("Artifacts panel"), "{error}");
    }

    #[test]
    fn delete_needs_approval_and_removes() {
        let w = wsfresh("del");
        std::fs::write(w.root().join("d.txt"), "bye").unwrap();
        let req = ToolRequest {
            name: "delete_file".into(),
            args: serde_json::json!({"path": "d.txt"}),
            approved: false,
        };
        assert!(matches!(
            execute(&req, &w, false).unwrap_err(),
            ToolError::PermissionRequired { .. }
        ));
        assert_eq!(risk_of("delete_file"), RiskLevel::Dangerous);
        let req = ToolRequest {
            name: "delete_file".into(),
            args: serde_json::json!({"path": "d.txt"}),
            approved: true,
        };
        assert!(execute(&req, &w, false).unwrap().ok);
        assert!(!w.root().join("d.txt").exists());
    }

    #[test]
    fn chat_safe_set_is_read_only() {
        for t in [
            "list_directory",
            "read_file",
            "search_text",
            "system_info",
            "list_processes",
        ] {
            assert!(chat_safe(t), "{t}");
        }
        for t in [
            "write_file",
            "edit_file",
            "delete_file",
            "execute_command",
            "git_commit",
            "open_path",
        ] {
            assert!(!chat_safe(t), "{t}");
        }
        // Documents never touch the project: they render into the app's own
        // artifacts folder, so chat may produce them without approval.
        assert!(chat_safe("create_document"));
    }

    #[test]
    fn git_commit_needs_approval_and_clean_message() {
        let w = wsfresh("gitc");
        let denied = ToolRequest {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "ok"}),
            approved: false,
        };
        assert!(matches!(
            execute(&denied, &w, false).unwrap_err(),
            ToolError::PermissionRequired { .. }
        ));
        let evil = ToolRequest {
            name: "git_commit".into(),
            args: serde_json::json!({"message": "x\"; rm -rf /"}),
            approved: true,
        };
        assert!(matches!(
            execute(&evil, &w, true).unwrap_err(),
            ToolError::InvalidArgs(_)
        ));
        assert_eq!(risk_of("git_commit"), RiskLevel::Moderate);
    }

    #[test]
    fn list_processes_returns_rows() {
        let w = wsfresh("procs");
        let req = ToolRequest {
            name: "list_processes".into(),
            args: serde_json::json!({}),
            approved: false,
        };
        let r = execute(&req, &w, false).unwrap();
        assert!(
            r.output.contains("pid "),
            "{}",
            r.output.chars().take(200).collect::<String>()
        );
    }
}
