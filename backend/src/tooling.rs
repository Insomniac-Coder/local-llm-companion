//! How each model calls tools, measured on this machine and kept next to the
//! model as `tooling.json`.
//!
//! A chat template that renders tools says what the model was trained on, not
//! what it does when this app prompts it. Measured on two models with the same
//! template (night decision 59): given the app's own text tool list, one wrote
//! native calls and the other copied the list's shape, `list_directory` then
//! `{"path":"."}`, which no parser read. Offered the same tools through the
//! runtime's tool interface, both called them correctly.
//!
//! So each model is checked once, at its first load: a native call through the
//! runtime, and a file whose text has to arrive byte for byte. When the native
//! path fails, the same two checks run in the app's text format. The result
//! decides how the agent offers tools to that model. It is checked again when
//! the model's chat template or the runtime build changes. No model name is
//! involved anywhere: the file is a measurement.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::inference::{InferenceConfig, Support, TemplateCaps};
use crate::llamaserver::{AgentCompletion, ChatTurn, SidecarClient};

pub const PROFILE_FILE: &str = "tooling.json";
/// Bumped when the checks change meaning, so older files are checked again.
pub const PROFILE_VERSION: u32 = 2;

/// How the agent offers tools to a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMethod {
    /// Tool definitions through the runtime; its parser returns the calls.
    Native,
    /// The app's text action format (a fenced JSON envelope).
    Text,
    /// Neither worked: chat and read-only code through the constrained JSON
    /// format (night decision 48).
    None,
}

/// Where a file's text travels in a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileText {
    /// Inside the call's arguments (the runtime constrains them to valid JSON).
    Arguments,
    /// In raw `<<<CONTENT` blocks after the text envelope's object.
    RawBlocks,
    /// No method delivered file text intact: the model does not write files.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub name: String,
    pub passed: bool,
    /// What the model sent, or why the check failed.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolingProfile {
    pub version: u32,
    pub method: ToolMethod,
    /// File text arrived intact, so the model may write and edit files.
    pub can_write: bool,
    pub file_text: FileText,
    /// RFC 3339.
    pub checked_at: String,
    /// The runtime build the checks ran on.
    pub runtime: String,
    /// The chat template the checks ran with (a fingerprint).
    pub template: String,
    /// What the runtime reported about the template, when it did.
    #[serde(default)]
    pub template_reports_tools: Option<bool>,
    pub checks: Vec<CheckOutcome>,
    pub duration_ms: u64,
}

impl ToolingProfile {
    /// One plain sentence for notices and the model card.
    pub fn summary(&self) -> String {
        match (self.method, self.can_write) {
            (ToolMethod::Native, true) => "calls tools natively through the runtime".into(),
            (ToolMethod::Text, true) => "calls tools through the app's text format".into(),
            (ToolMethod::Native | ToolMethod::Text, false) => {
                "can read files with tools, but file text did not arrive intact, so code sessions stay read-only".into()
            }
            (ToolMethod::None, _) => {
                "could not call tools in either format, so it gets chat and read-only code".into()
            }
        }
    }
}

/// Whether a model's profile still describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileState {
    Current,
    /// Checked, but with another runtime build or chat template.
    Stale,
    Unchecked,
}

pub fn profile_state(profile: Option<&ToolingProfile>, runtime: &str, template: &str) -> ProfileState {
    match profile {
        None => ProfileState::Unchecked,
        Some(profile) if profile.version == PROFILE_VERSION && profile.runtime == runtime && profile.template == template => {
            ProfileState::Current
        }
        Some(_) => ProfileState::Stale,
    }
}

/// The folder a model's profile lives in: the folder of its weights.
pub fn profile_dir(model_path: &Path) -> PathBuf {
    model_path.parent().map(Path::to_path_buf).unwrap_or_default()
}

pub fn read_profile(dir: &Path) -> Option<ToolingProfile> {
    let text = std::fs::read_to_string(dir.join(PROFILE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Written beside the model, through a temporary file so a reader never sees
/// half of it.
pub fn write_profile(dir: &Path, profile: &ToolingProfile) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(profile).map_err(std::io::Error::other)?;
    let temporary = dir.join(format!("{PROFILE_FILE}.tmp"));
    std::fs::write(&temporary, text + "\n")?;
    std::fs::rename(&temporary, dir.join(PROFILE_FILE))
}

/// The runtime build, from the `BUILD_INFO.json` the build script writes next
/// to the server binary. A binary without one (an override) is identified by
/// its size and modification time, so replacing it still counts as a change.
pub fn runtime_identity(server_binary: &Path) -> String {
    let info = server_binary
        .parent()
        .map(|dir| dir.join("BUILD_INFO.json"))
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()
        });
    if let Some(info) = info {
        let tag = info.get("tag").and_then(|tag| tag.as_str()).unwrap_or("");
        let commit = info.get("commit").and_then(|commit| commit.as_str()).unwrap_or("");
        if !commit.is_empty() {
            return format!("{tag} {}", &commit[..commit.len().min(12)]).trim().to_string();
        }
    }
    match std::fs::metadata(server_binary) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time| time.as_secs())
                .unwrap_or(0);
            format!("unlabelled build {} bytes {modified}", meta.len())
        }
        Err(_) => "unknown runtime".into(),
    }
}

/// A short fingerprint of the chat template text a model is served with.
pub fn template_fingerprint(template: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(template.as_bytes());
    digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// Parameter schemas for each tool's arguments, in the OpenAI function
/// format the runtime accepts. Required keys follow `tools::required_args`.
fn parameters(tool: &str) -> serde_json::Value {
    let string = |description: &str| serde_json::json!({"type": "string", "description": description});
    let integer = |description: &str| serde_json::json!({"type": "integer", "description": description});
    let boolean = |description: &str| serde_json::json!({"type": "boolean", "description": description});
    let path = string("Path relative to the workspace root");
    let (properties, required): (serde_json::Value, &[&str]) = match tool {
        "list_directory" => (serde_json::json!({"path": string("Folder relative to the workspace root; \".\" is the root")}), &[]),
        "read_file" => (
            serde_json::json!({
                "path": path,
                "start_line": integer("First line to read, 1-based"),
                "end_line": integer("Last line to read"),
                "start_column": integer("Column to continue an unusually long line from"),
            }),
            &["path"],
        ),
        "write_file" => (serde_json::json!({"path": path, "content": string("The complete text of the file")}), &["path", "content"]),
        "append_file" => (serde_json::json!({"path": path, "content": string("The text to add at the end of the file")}), &["path", "content"]),
        "edit_file" => (
            serde_json::json!({
                "path": path,
                "old": string("The exact text to replace, copied from the file"),
                "new": string("The text that replaces it"),
            }),
            &["path", "old", "new"],
        ),
        "replace_lines" => (
            serde_json::json!({
                "path": path,
                "from": integer("First line to replace, as read_file numbers them (1-based)"),
                "to": integer("Last line to replace; the same as from for one line"),
                "text": string("The lines that take their place; empty deletes them"),
                "expect": string("The text of line `from` as you read it, checked before anything is replaced"),
            }),
            &["path", "from", "to", "text"],
        ),
        "outline" => (
            serde_json::json!({"path": string("A file to list the definitions of, or a folder to list; \".\" for the workspace root")}),
            &["path"],
        ),
        "project_check" => (
            serde_json::json!({
                "path": string("Folder holding the project; \".\" for the workspace root"),
                "timeout_secs": integer("Seconds before a build or test run is stopped"),
            }),
            &[],
        ),
        "delete_file" | "open_path" => (serde_json::json!({"path": path}), &["path"]),
        "search_text" => (
            serde_json::json!({"query": string("Regular expression to search for"), "path": string("Folder or file to search; \".\" for everything")}),
            &["query"],
        ),
        "execute_command" => (
            serde_json::json!({
                "command": string("The command line"),
                "cwd": string("Folder to run it in, relative to the workspace root"),
                "timeout_secs": integer("Seconds before the command is stopped"),
                "background": boolean("True for a command that is meant to keep running, such as a dev server: it stays up and its output is kept for you"),
                "wait_secs": integer("Seconds to watch a background command before reporting what it printed"),
                "background_id": integer("Read a background command already started, instead of starting one"),
                "stop": boolean("With background_id: stop that command"),
            }),
            &["command"],
        ),
        "preview_page" => (
            serde_json::json!({
                "url": string("The address the project's server prints, such as http://localhost:5173"),
                "wait_secs": integer("Seconds to wait for the server to answer"),
                "screenshot": boolean("Also save a picture of the page to a file"),
            }),
            &["url"],
        ),
        "web_search" => (serde_json::json!({"query": string("Search terms")}), &["query"]),
        "remember" => (
            serde_json::json!({
                "note": string("One short fact to keep in front of you, such as a decision, a value you looked up, or a convention this project follows"),
                "replace": string("Text of an earlier note this one supersedes"),
            }),
            &["note"],
        ),
        "git_commit" => (serde_json::json!({"message": string("Commit message")}), &["message"]),
        "present_plan" => (
            serde_json::json!({"plan": string("The plan in Markdown: which files change and the steps in order")}),
            &[],
        ),
        "create_document" => (
            serde_json::json!({
                "filename": string("File name with its extension: xlsx, docx, pdf, pptx, md, txt, csv, html or json"),
                "title": string("Title (docx, pdf, pptx, html)"),
                "paragraphs": {"type": "array", "items": {"type": "string"}, "description": "Paragraphs (docx, pdf)"},
                "sheets": {"type": "array", "description": "Sheets (xlsx): objects with name and rows"},
                "slides": {"type": "array", "description": "Slides (pptx): objects with title and bullets"},
                "rows": {"type": "array", "description": "Rows (csv)"},
                "text": string("Text (md, txt)"),
                "html": string("Body HTML (html)"),
                "data": {"description": "Data (json)"},
            }),
            &["filename"],
        ),
        _ => (serde_json::json!({}), &[]),
    };
    serde_json::json!({"type": "object", "properties": properties, "required": required})
}

/// Tool definitions for the runtime's tool interface.
pub fn tool_definitions(tools: &[crate::tools::ToolDescriptor]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": parameters(tool.name),
                },
            })
        })
        .collect()
}

fn descriptors(names: &[&str]) -> Vec<crate::tools::ToolDescriptor> {
    crate::tools::registry()
        .into_iter()
        .filter(|tool| names.contains(&tool.name))
        .collect()
}

/// Text a file check asks for: quotes, backslashes, a tab, braces, a literal
/// `\n` and non-ASCII characters, the characters that broke writes before.
/// A small Python file holding the characters that broke writes before:
/// double and single quotes, backslashes in a Windows path, a `\n` escape
/// inside a string, a tab-indented line, JSON-like braces and non-ASCII
/// text. Real code, so no part of it reads like a label a model might drop
/// (the first version had "first line with" and "last line:", which one
/// model left out as labels in both formats while every backslash arrived
/// intact: night decision 59).
pub const PROBE_FILE_TEXT: &str = "def describe(order):\n\tpath = \"C:\\\\orders\\\\new.json\"\n\tnote = \"it's \\\"ready\\\"\"\n\treturn f\"{path}\\n{note}\", {\"total\": [1, 2], \"city\": \"São Paulo\", \"temp\": \"25°C\"}";

const PROBE_SYSTEM: &str = "You are a coding assistant working inside a project folder. Use the available tools to inspect and change files, one tool call at a time. Paths are relative to the project folder.";
const PROBE_LIST_TASK: &str = "Which files are in the project folder? Look before you answer.";

fn probe_write_task() -> String {
    format!("Create the file probe.py containing exactly this Python code, character for character, with nothing added, removed or changed:\n\n```python\n{PROBE_FILE_TEXT}\n```")
}

/// A reply's visible text that still carries call syntax: the runtime did not
/// read the call, so the host would have to.
fn leaked_call_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    text.contains("<|tool_call>")
        || text.contains("<tool_call>")
        || text.contains("```tool")
        || (trimmed.starts_with('{') && trimmed.contains("\"name\""))
        || crate::tools::registry().iter().any(|tool| {
            trimmed
                .strip_prefix(tool.name)
                .is_some_and(|rest| rest.trim_start().starts_with('{'))
        })
}

fn same_text(sent: &str, expected: &str) -> bool {
    let sent = sent.strip_suffix('\n').unwrap_or(sent);
    let sent = sent.replace("\r\n", "\n");
    sent == expected
}

fn shorten(text: &str) -> String {
    let flat: String = text.chars().take(240).collect();
    if text.chars().count() > 240 {
        format!("{flat}…")
    } else {
        flat
    }
}

fn first_difference(sent: &str, expected: &str) -> String {
    let sent = sent.strip_suffix('\n').unwrap_or(sent);
    let position = sent
        .chars()
        .zip(expected.chars())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| sent.chars().count().min(expected.chars().count()));
    let around = |text: &str| text.chars().skip(position.saturating_sub(12)).take(28).collect::<String>();
    format!(
        "the text differs from character {}: sent {:?}, expected {:?}",
        position + 1,
        around(sent),
        around(expected)
    )
}

type Probe = Result<AgentCompletion, crate::inference::thiserror_stub::InferenceError>;

fn native_call_outcome(result: &Probe) -> CheckOutcome {
    let name = "native_call".to_string();
    let completion = match result {
        Ok(completion) => completion,
        Err(error) => return CheckOutcome { name, passed: false, detail: format!("the request failed: {error}") },
    };
    let Some(call) = completion.tool_calls.first() else {
        return CheckOutcome {
            name,
            passed: false,
            detail: if completion.text.trim().is_empty() {
                "no tool call and no text came back".into()
            } else {
                format!("no tool call came back; the reply was: {}", shorten(completion.text.trim()))
            },
        };
    };
    let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments);
    let detail = format!("{} {}", call.name, shorten(&call.arguments));
    let passed = matches!(call.name.as_str(), "list_directory" | "read_file")
        && arguments.as_ref().is_ok_and(|value| value.is_object())
        && !leaked_call_text(&completion.text);
    CheckOutcome {
        name,
        passed,
        detail: if passed {
            detail
        } else if leaked_call_text(&completion.text) {
            format!("{detail}, but call text also came back in the reply: {}", shorten(&completion.text))
        } else {
            format!("{detail}: not a listing or reading call with an object of arguments")
        },
    }
}

fn file_outcome(name: &str, call: Option<(String, serde_json::Value)>, result: &Probe) -> CheckOutcome {
    let name = name.to_string();
    if let Err(error) = result {
        return CheckOutcome { name, passed: false, detail: format!("the request failed: {error}") };
    }
    let Some((tool, args)) = call else {
        let text = result.as_ref().map(|completion| completion.text.trim().to_string()).unwrap_or_default();
        return CheckOutcome {
            name,
            passed: false,
            detail: if text.is_empty() { "no write call came back".into() } else { format!("no write call came back; the reply was: {}", shorten(&text)) },
        };
    };
    let content = args.get("content").and_then(|content| content.as_str());
    match content {
        _ if tool != "write_file" => CheckOutcome { name, passed: false, detail: format!("called {tool} instead of write_file") },
        None => CheckOutcome { name, passed: false, detail: "write_file came back without its content".into() },
        Some(content) if same_text(content, PROBE_FILE_TEXT) => CheckOutcome {
            name,
            passed: true,
            detail: format!("write_file with all {} characters intact", PROBE_FILE_TEXT.chars().count()),
        },
        Some(content) => CheckOutcome { name, passed: false, detail: first_difference(content, PROBE_FILE_TEXT) },
    }
}

/// A native call's name and arguments, when they parse.
fn native_write(result: &Probe) -> Option<(String, serde_json::Value)> {
    let call = result.as_ref().ok()?.tool_calls.first()?;
    let args = serde_json::from_str::<serde_json::Value>(&call.arguments).ok()?;
    Some((call.name.clone(), args))
}

fn text_write(result: &Probe) -> Option<(String, serde_json::Value)> {
    let call = crate::agent_runner::parse_tool_block(&result.as_ref().ok()?.text)?;
    Some((call.name, call.args))
}

fn text_call_outcome(result: &Probe) -> CheckOutcome {
    let name = "text_call".to_string();
    let completion = match result {
        Ok(completion) => completion,
        Err(error) => return CheckOutcome { name, passed: false, detail: format!("the request failed: {error}") },
    };
    match crate::agent_runner::parse_tool_block(&completion.text) {
        Some(call) if matches!(call.name.as_str(), "list_directory" | "read_file") => CheckOutcome {
            name,
            passed: true,
            detail: format!("{} {}", call.name, shorten(&call.args.to_string())),
        },
        Some(call) => CheckOutcome { name, passed: false, detail: format!("called {} instead of listing or reading", call.name) },
        None => CheckOutcome {
            name,
            passed: false,
            detail: format!("no readable action; the reply was: {}", shorten(completion.text.trim())),
        },
    }
}

/// Run the checks against the loaded model. `Err` when the runtime could not
/// answer at all, so no profile is written from a failed connection.
pub async fn check(
    client: &SidecarClient,
    cfg: &InferenceConfig,
    runtime: String,
    template: String,
    caps: Option<TemplateCaps>,
) -> Result<ToolingProfile, String> {
    let started = Instant::now();
    let mut checks = Vec::new();

    let reading = tool_definitions(&descriptors(&["list_directory", "read_file"]));
    let writing = tool_definitions(&descriptors(&["write_file"]));
    let turns = |task: String| vec![ChatTurn::text("system", PROBE_SYSTEM), ChatTurn::text("user", task)];

    let native_call = client.probe_turns(&turns(PROBE_LIST_TASK.into()), 256, cfg, Some(&reading)).await;
    if let Err(error) = &native_call {
        if error.sidecar().is_some_and(|failure| failure.is_transient()) {
            return Err(format!("the runtime did not answer the tool check: {error}"));
        }
    }
    checks.push(native_call_outcome(&native_call));
    let native_file = client.probe_turns(&turns(probe_write_task()), 768, cfg, Some(&writing)).await;
    checks.push(file_outcome("native_file_text", native_write(&native_file), &native_file));
    let native_ok = checks.iter().all(|check| check.passed);

    let (mut method, mut can_write, mut file_text) = if native_ok {
        (ToolMethod::Native, true, FileText::Arguments)
    } else {
        (ToolMethod::None, false, FileText::None)
    };
    if !native_ok {
        // The same two checks in the app's text format.
        let text_turns = |tools: &[&str], task: &str| {
            let prompt = crate::agent_runner::system_prompt_for(".", &descriptors(tools), false, false, false, 12_000, false, 32_768);
            vec![
                ChatTurn::text("system", prompt),
                ChatTurn::text("user", format!("Task: {task}\n\nUse the complete tool envelope shown in the system instructions for each action.")),
            ]
        };
        let text_call = client.probe_turns(&text_turns(&["list_directory", "read_file"], PROBE_LIST_TASK), 256, cfg, None).await;
        checks.push(text_call_outcome(&text_call));
        let text_file = client.probe_turns(&text_turns(&["write_file"], &probe_write_task()), 768, cfg, None).await;
        checks.push(file_outcome("text_file_text", text_write(&text_file), &text_file));
        let passed = |name: &str| checks.iter().any(|check| check.name == name && check.passed);
        (method, can_write, file_text) = if passed("text_call") && passed("text_file_text") {
            (ToolMethod::Text, true, FileText::RawBlocks)
        } else if passed("native_call") {
            (ToolMethod::Native, false, FileText::None)
        } else if passed("text_call") {
            (ToolMethod::Text, false, FileText::None)
        } else {
            (ToolMethod::None, false, FileText::None)
        };
    }

    Ok(ToolingProfile {
        version: PROFILE_VERSION,
        method,
        can_write,
        file_text,
        checked_at: chrono::Utc::now().to_rfc3339(),
        runtime,
        template,
        template_reports_tools: caps.and_then(|caps| match caps.tools_supported() {
            Support::Yes => Some(true),
            Support::No => Some(false),
            Support::Unknown => None,
        }),
        checks,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llamaserver::NativeToolCall;

    fn completion(text: &str, calls: &[(&str, &str)]) -> Probe {
        Ok(AgentCompletion {
            text: text.into(),
            metrics: Default::default(),
            finish_reason: Some("stop".into()),
            reasoning_present: false,
            reasoning_tokens: None,
            native_tool_calls_present: !calls.is_empty(),
            tool_calls: calls
                .iter()
                .enumerate()
                .map(|(index, (name, arguments))| NativeToolCall { id: format!("call_{index}"), name: (*name).into(), arguments: (*arguments).into() })
                .collect(),
            early_stopped: false,
        })
    }

    #[test]
    fn a_parsed_listing_call_passes_and_the_shapes_that_failed_do_not() {
        assert!(native_call_outcome(&completion("", &[("list_directory", "{\"path\":\".\"}")])).passed);
        assert!(native_call_outcome(&completion("Let me look first.", &[("read_file", "{\"path\":\"README.md\"}")])).passed, "prose beside a call is fine");
        // Measured (night decision 59): the tool list's shape, as plain text.
        let copied = native_call_outcome(&completion("list_directory\n{\"path\":\".\"}", &[]));
        assert!(!copied.passed && copied.detail.contains("no tool call"), "{}", copied.detail);
        let leaked = native_call_outcome(&completion("<|tool_call>call:list_directory{}<tool_call|>", &[("list_directory", "{}")]));
        assert!(!leaked.passed && leaked.detail.contains("call text"), "{}", leaked.detail);
        assert!(!native_call_outcome(&completion("", &[("list_directory", "not json")])).passed);
        assert!(!native_call_outcome(&completion("", &[("write_file", "{}")])).passed);
        assert!(!native_call_outcome(&completion("", &[])).passed);
    }

    #[test]
    fn file_text_must_arrive_exactly_apart_from_a_final_newline() {
        let arguments = serde_json::json!({"path": "probe.txt", "content": PROBE_FILE_TEXT}).to_string();
        let exact = completion("", &[("write_file", &arguments)]);
        assert!(file_outcome("native_file_text", native_write(&exact), &exact).passed);
        let with_newline = serde_json::json!({"path": "probe.txt", "content": format!("{PROBE_FILE_TEXT}\n")}).to_string();
        let newline = completion("", &[("write_file", &with_newline)]);
        assert!(file_outcome("native_file_text", native_write(&newline), &newline).passed);
        // A backslash sequence decoded on the way is exactly the damage checked for.
        let damaged = serde_json::json!({"path": "probe.py", "content": PROBE_FILE_TEXT.replace("}\\n{", "}\n{")}).to_string();
        let damaged = completion("", &[("write_file", &damaged)]);
        let outcome = file_outcome("native_file_text", native_write(&damaged), &damaged);
        assert!(!outcome.passed && outcome.detail.contains("differs from character"), "{}", outcome.detail);
        let text_format = completion(&format!("```tool\n{{\"name\":\"write_file\",\"args\":{{\"path\":\"probe.txt\"}}}}\n<<<CONTENT\n{PROBE_FILE_TEXT}\nCONTENT>>>\n```"), &[]);
        assert!(file_outcome("text_file_text", text_write(&text_format), &text_format).passed);
    }

    #[test]
    fn a_profile_is_current_only_for_the_runtime_and_template_it_was_checked_with() {
        let dir = std::env::temp_dir().join(format!("tooling-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(profile_state(read_profile(&dir).as_ref(), "b1 abc", "t1"), ProfileState::Unchecked);
        let profile = ToolingProfile {
            version: PROFILE_VERSION,
            method: ToolMethod::Native,
            can_write: true,
            file_text: FileText::Arguments,
            checked_at: "2026-09-17T00:00:00Z".into(),
            runtime: "b1 abc".into(),
            template: "t1".into(),
            template_reports_tools: Some(true),
            checks: vec![],
            duration_ms: 1,
        };
        write_profile(&dir, &profile).unwrap();
        let read = read_profile(&dir).unwrap();
        assert_eq!(read, profile);
        assert!(!dir.join("tooling.json.tmp").exists());
        assert_eq!(profile_state(Some(&read), "b1 abc", "t1"), ProfileState::Current);
        assert_eq!(profile_state(Some(&read), "b2 def", "t1"), ProfileState::Stale);
        assert_eq!(profile_state(Some(&read), "b1 abc", "t2"), ProfileState::Stale);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_runtime_is_identified_by_its_build_info_or_the_binary_itself() {
        let dir = std::env::temp_dir().join(format!("tooling-rt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("llama-server");
        std::fs::write(&binary, b"bin").unwrap();
        assert!(runtime_identity(&binary).starts_with("unlabelled build 3 bytes"));
        // The build script writes it with a byte-order mark.
        std::fs::write(dir.join("BUILD_INFO.json"), "\u{feff}{\"tag\": \"b10809\", \"commit\": \"5266f24da75dc449bd56\"}").unwrap();
        assert_eq!(runtime_identity(&binary), "b10809 5266f24da75d");
        assert_eq!(template_fingerprint("{{ messages }}").len(), 16);
        assert_ne!(template_fingerprint("a"), template_fingerprint("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_tool_gets_a_definition_with_its_required_arguments() {
        let definitions = tool_definitions(&crate::tools::registry());
        assert_eq!(definitions.len(), crate::tools::registry().len());
        for definition in &definitions {
            let function = &definition["function"];
            let name = function["name"].as_str().unwrap();
            let properties = function["parameters"]["properties"].as_object().unwrap();
            for (argument, _) in crate::tools::required_args(name) {
                assert!(properties.contains_key(*argument), "{name} lacks {argument}");
            }
            for required in function["parameters"]["required"].as_array().unwrap() {
                assert!(properties.contains_key(required.as_str().unwrap()), "{name} requires an undefined {required}");
            }
        }
    }
}
