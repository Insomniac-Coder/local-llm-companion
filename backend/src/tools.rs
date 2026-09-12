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
            name: "edit_file",
            description: "Apply a unified-diff patch to a workspace file",
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
        "write_file" | "edit_file" | "web_search" | "create_document" | "git_commit"
        | "open_path" => RiskLevel::Moderate,
        _ => RiskLevel::Safe,
    }
}

/// Tool names the in-chat loop may run WITHOUT asking (Stage 31): read-only,
/// workspace-confined, no side effects. Everything else needs the agent
/// panel/commands approval flow.
pub fn chat_safe(name: &str) -> bool {
    matches!(
        name,
        "list_directory" | "read_file" | "search_text" | "system_info" | "list_processes"
    )
}

const MAX_FILE_BYTES: u64 = 5_000_000;
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
            Ok(ToolResult::ok(format!("wrote {} bytes to {rel}", content.len())))
        }
        "edit_file" => {
            require_approved(req, approved, "File modifications need explicit approval.")?;
            let rel = req.args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("edit_file requires {\"path\": \"...\", \"patch\": \"--- ...\"}".into())
            })?;
            let patch = req.args.get("patch").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidArgs("edit_file requires {\"path\": \"...\", \"patch\": \"--- ...\"}".into())
            })?;
            let p = ws.resolve(rel).map_err(ToolError::Workspace)?;
            let original = std::fs::read_to_string(&p).map_err(ToolError::Io)?;
            if original.len() as u64 > MAX_FILE_BYTES {
                return Err(ToolError::InvalidArgs("file too large to patch; rewrite it in chunks".into()));
            }
            let updated = apply_unified_patch(&original, patch)
                .map_err(ToolError::InvalidArgs)?;
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
    let orig: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = vec![];
    let mut orig_idx = 0usize; // 0-based cursor into orig
    let mut hunk_count = 0usize;
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("---") || line.starts_with("+++") {
            i += 1;
            continue;
        }
        if !line.starts_with("@@") {
            return Err(format!("expected '@@' hunk header, got: {line}"));
        }
        hunk_count += 1;
        let (old_start, old_len) = parse_hunk_header(line)?;
        // Copy unchanged lines between the end of the last hunk and this one.
        // old_start is 1-based; converting to 0-based cursor position.
        let want_idx = old_start.saturating_sub(1);
        if want_idx < orig_idx {
            return Err("overlapping hunks".into());
        }
        while orig_idx < want_idx {
            if orig_idx >= orig.len() {
                return Err("hunk starts past end of file".into());
            }
            out.push(orig[orig_idx].to_string());
            orig_idx += 1;
        }
        i += 1;
        let mut consumed_old = 0usize;
        while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("---") {
            let hunk_line = lines[i];
            let (kind, text) = hunk_line.split_at(1.min(hunk_line.len()));
            match kind {
                " " => {
                    if orig.get(orig_idx) != Some(&text) {
                        return Err(format!(
                            "context mismatch at line {}: expected {text:?}, found {:?}. Re-read the file and retry.",
                            orig_idx + 1,
                            orig.get(orig_idx)
                        ));
                    }
                    out.push(text.to_string());
                    orig_idx += 1;
                    consumed_old += 1;
                }
                "-" => {
                    if orig.get(orig_idx) != Some(&text) {
                        return Err(format!(
                            "removal mismatch at line {}: expected {text:?}, found {:?}. Re-read the file and retry.",
                            orig_idx + 1,
                            orig.get(orig_idx)
                        ));
                    }
                    orig_idx += 1;
                    consumed_old += 1;
                }
                "+" => {
                    out.push(text.to_string());
                }
                _ => {
                    return Err(format!(
                        "bad hunk line: {hunk_line:?} (must start with ' ', '-', '+')"
                    ))
                }
            }
            i += 1;
        }
        if consumed_old != old_len {
            return Err(format!(
                "hunk consumed {consumed_old} original lines but header claims {old_len}"
            ));
        }
    }
    if hunk_count == 0 {
        return Err("patch contains no hunks".into());
    }
    // Copy the tail. Preserve a trailing newline iff the original had one.
    while orig_idx < orig.len() {
        out.push(orig[orig_idx].to_string());
        orig_idx += 1;
    }
    let mut result = out.join("\n");
    if original.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
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
        assert!(err.contains("mismatch"), "{err}");
        assert!(apply_unified_patch(original, "no hunks here").is_err());
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
            "create_document",
        ] {
            assert!(!chat_safe(t), "{t}");
        }
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
