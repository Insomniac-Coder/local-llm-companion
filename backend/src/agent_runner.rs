//! LLM coding-agent loop (§28–30, §45, §82, §94).
//!
//! Each iteration: model reasons over the transcript → emits at most one
//! ```tool fenced call (or a final answer) → gate → execute → observe.
//! Bounded by max_iterations and a shared no-progress budget. Approvals pause
//! the loop (§26); Stop cancels it and aborts any in-flight sidecar request.

use crate::agent::{AgentEvent, AgentLimits, AgentMode, AgentState, CancelToken, PendingTool};
use crate::llamaserver::{ChatTurn, SidecarClient, StreamHandlers};
use crate::permissions::{PermissionDecision, RiskLevel};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

/// Soft cap on transcript turns; the size-aware pruning below is the real
/// bound. Prefix caching makes a long unchanged transcript nearly free.
const TRANSCRIPT_TURNS: usize = 60;
const TOOL_OUTPUT_CHARS: usize = 16_000;

/// Characters of one tool result kept in the transcript.
///
/// The ceiling alone was a third of the usable room on a 16K window and more
/// than all of it on a 5K one, so a single directory listing or file read
/// could take the transcript from below the compaction threshold to past the
/// room in one step: the threshold was then met only after it had been
/// overshot, and the user saw 112% where the limit was 90%. Bounding a result
/// to a fifth of the room bounds that overshoot instead.
///
/// The floor keeps a result readable on the smallest windows; a result is cut
/// at the end and the model can read the file again for the rest. Four
/// characters per token matches the transcript estimate this bounds.
fn tool_output_chars(room_tokens: u32) -> usize {
    let room_chars = room_tokens as usize * 4;
    TOOL_OUTPUT_CHARS.min((room_chars / 5).max(1_200))
}
const RELEASED_MARKER: &str = "[Released:";

#[derive(Default)]
struct ActionResponsePolicy {
    disable_native_thinking: bool,
    invalid_since_progress: u32,
    continuation_used: bool,
    structured_fallback: bool,
}

impl ActionResponsePolicy {
    /// Enough for a complete file write in one action. A complete action ends
    /// the stream early, so a large allowance costs nothing on short replies.
    fn output_cap(&self) -> u32 {
        if self.disable_native_thinking {
            8192
        } else {
            4096
        }
    }

    fn output_budget(&self, context: u32, estimated_input: u32) -> u32 {
        self.output_cap()
            .min(context.saturating_sub(estimated_input.saturating_add(256)))
    }

    /// Two changed-strategy recoveries, never unlimited rephrasing of a
    /// failure: first a native retry with the format reminder and thinking
    /// off, then the schema-constrained envelope.
    fn recover_invalid(&mut self) -> bool {
        self.invalid_since_progress += 1;
        self.disable_native_thinking = true;
        self.structured_fallback = self.invalid_since_progress >= 2;
        self.invalid_since_progress <= 2
    }

    fn begin_continuation(&mut self) -> bool {
        if self.continuation_used || self.invalid_since_progress > 0 {
            return false;
        }
        self.continuation_used = true;
        self.disable_native_thinking = true;
        true
    }

    fn useful_progress(&mut self) {
        self.invalid_since_progress = 0;
        self.continuation_used = false;
        // Keep the working template override for this run, not global Chat.
    }
}

fn action_response_invalid_reason(
    completion: &crate::llamaserver::AgentCompletion,
    parsed_call: Option<&ToolCall>,
    output_budget: u32,
) -> Option<String> {
    if completion.is_truncated() {
        Some(format!(
            "The response reached its {output_budget}-token output limit before completing."
        ))
    } else if completion.native_tool_calls_present {
        Some("The model used an unsupported action format.".into())
    } else if completion.text.trim().is_empty() {
        Some(
            if completion.reasoning_present {
                "The model produced reasoning but no visible action or answer."
            } else {
                "The model returned no visible action or answer."
            }
            .into(),
        )
    } else if parsed_call.is_none() && looks_like_action_attempt(&completion.text) {
        Some("The model returned an incomplete or unreadable action.".into())
    } else if parsed_call.is_none()
        && completion.text.trim_start().starts_with('{')
        && parse_structured_reply(&completion.text).is_none()
    {
        Some("The model returned an invalid structured action or final answer.".into())
    } else {
        None
    }
}

fn can_continue_response(completion: &crate::llamaserver::AgentCompletion) -> bool {
    completion.is_truncated()
        && !completion.native_tool_calls_present
        && !completion.text.trim().is_empty()
        && completion.text.chars().count() <= 32_768
}

fn continuation_turns(transcript: &[ChatTurn], prefix: &str) -> Vec<ChatTurn> {
    let mut turns = transcript.to_vec();
    turns.push(ChatTurn::text("assistant", prefix));
    turns.push(ChatTurn::text("user", "The previous response was cut off by the output limit. Continue it from the exact next character. Return ONLY the missing suffix: do not repeat the prefix, add an introduction, or open a new tool block. If inside a JSON string, continue that string with correct JSON escaping, then close the JSON and its existing fence or native tool envelope. Do not change formats. Complete only that one response/action. Nothing from the partial response has been executed."));
    turns
}

fn assemble_continuation(prefix: &str, suffix: &str) -> Option<String> {
    if suffix.trim().is_empty() {
        return None;
    }
    // Some models repeat the entire prefix. Accept only an exact extension;
    // never guess overlap boundaries that could corrupt source code or JSON.
    let combined = if suffix.starts_with(prefix) {
        if suffix.len() == prefix.len() {
            return None;
        }
        suffix.to_owned()
    } else {
        format!("{prefix}{suffix}")
    };
    (combined.chars().count() <= 65_536).then_some(combined)
}

/// Structured call the model emits inside a ```tool fence (§31).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredReply {
    kind: String,
    name: String,
    args: serde_json::Value,
    answer: String,
}

/// Repair the JSON slips small models make whose intent is unambiguous:
/// raw newlines and tabs inside string values (a multi-line `old` or
/// `content`), a trailing comma before `]` or `}`, and an unquoted
/// spreadsheet formula such as `=SUM(C2:C6)` in value position. Text inside
/// strings is otherwise untouched and structurally broken JSON still fails.
fn repair_json_strings(body: &str) -> std::borrow::Cow<'_, str> {
    if !body.contains(['\n', '\r', '\t', ',', '=', '\\']) {
        return std::borrow::Cow::Borrowed(body);
    }
    let mut out = String::with_capacity(body.len() + 16);
    let mut in_string = false;
    let mut changed = false;
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_string {
            match ch {
                '\\' => match chars.peek().copied() {
                    Some(next @ ('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u')) => {
                        out.push('\\');
                        out.push(next);
                        chars.next();
                    }
                    // Quotes and backticks escaped for JavaScript or Python,
                    // which JSON forbids: `querySelector(\'.hero\')` and a
                    // template literal opened with \` were meant as plain
                    // delimiters. A rejected page script was exactly this.
                    Some(next @ ('\'' | '`')) => {
                        out.push(next);
                        chars.next();
                        changed = true;
                    }
                    // Any other backslash is one the file needs (a regex, a
                    // Windows path): keep it literally.
                    Some(_) => {
                        out.push_str("\\\\");
                        changed = true;
                    }
                    None => out.push('\\'),
                },
                '"' => {
                    in_string = false;
                    out.push(ch);
                }
                '\n' => {
                    changed = true;
                    out.push_str("\\n");
                }
                '\r' => {
                    changed = true;
                    out.push_str("\\r");
                }
                '\t' => {
                    changed = true;
                    out.push_str("\\t");
                }
                _ => out.push(ch),
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            ']' | '}' => {
                // A trailing comma: drop it.
                let trimmed = out.trim_end_matches(|c: char| c.is_whitespace());
                if trimmed.ends_with(',') {
                    let keep = trimmed.len() - 1;
                    out.truncate(keep);
                    changed = true;
                }
                out.push(ch);
            }
            '=' if out
                .trim_end()
                .chars()
                .next_back()
                .is_some_and(|prev| matches!(prev, ':' | ',' | '[')) =>
            {
                // An unquoted formula in value position: quote it up to the
                // next delimiter outside its parentheses.
                let mut token = String::from("=");
                let mut depth = 0i32;
                while let Some(&next) = chars.peek() {
                    match next {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        ',' | ']' | '}' if depth <= 0 => break,
                        '\n' | '\r' => break,
                        _ => {}
                    }
                    token.push(next);
                    chars.next();
                }
                out.push('"');
                out.push_str(&token.trim_end().replace('"', "\\\""));
                out.push('"');
                changed = true;
            }
            _ => out.push(ch),
        }
    }
    if changed {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(body)
    }
}

fn parse_structured_reply(text: &str) -> Option<StructuredReply> {
    let repaired = repair_json_strings(text);
    let reply: StructuredReply = serde_json::from_str(&repaired).ok()?;
    if !reply.args.is_object() {
        return None;
    }
    match reply.kind.as_str() {
        // `kind` is authoritative. A tool's optional answer is a note, never a
        // completion; unused tool metadata in a final response never executes.
        "tool" if !reply.name.trim().is_empty() => Some(reply),
        "final" if !reply.answer.trim().is_empty() => Some(reply),
        _ => None,
    }
}

const STRUCTURED_ACTION_INSTRUCTION: &str = "For this response use the enforced JSON envelope, not Markdown or native tool-call notation. Return exactly one object with all four fields: kind, name, args, answer. For an action use kind=tool, the registered tool name, its complete argument object (every required argument, for example path for file tools), and an empty answer string. For a final response use kind=final, an empty name string, an empty args object, and your nonempty final answer string. No extra fields or text. This format changes no task scope, tool permissions, or verification requirements.";

/// One action found in a model reply: the call, the byte span it occupies,
/// and the closing marker to append when the model ended its turn before
/// writing one (so the transcript copy stays well-formed for later turns).
struct LocatedAction {
    call: ToolCall,
    span: std::ops::Range<usize>,
    missing_close: Option<&'static str>,
}

/// A line that opens a fence: `Some(true)` for the fences a model may wrap an
/// action in (the documented `tool` label, plus the `json` and bare fences
/// smaller models substitute freely), `Some(false)` for ordinary code.
fn fence_opener(line: &str) -> Option<bool> {
    let plain = line.trim_end();
    let indent = plain.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let label = plain[indent..].strip_prefix("```")?.trim();
    Some(matches!(
        label.to_ascii_lowercase().as_str(),
        "tool" | "json" | ""
    ))
}

fn known_tool(name: &str) -> bool {
    crate::tools::registry().iter().any(|tool| tool.name == name)
}

/// The first JSON object in `body` read as an action: `name` (or `tool`) plus
/// object `args` (`arguments`/`parameters` accepted; absent or null means
/// `{}`, and the tool's own validation then reports exactly what it needs).
/// Returns the call, the bytes consumed and whether the object carried a
/// `kind` envelope. A `kind: final` envelope is an answer, never a call.
fn parse_action_object(body: &str) -> Option<(ToolCall, usize, bool)> {
    let mut values =
        serde_json::Deserializer::from_str(body).into_iter::<serde_json::Value>();
    let value = values.next()?.ok()?;
    let consumed = values.byte_offset();
    let object = value.as_object()?;
    let enveloped = object.contains_key("kind");
    if object.get("kind").and_then(|kind| kind.as_str()) == Some("final") {
        return None;
    }
    let name = object
        .get("name")
        .or_else(|| object.get("tool"))
        .and_then(|name| name.as_str())?
        .trim();
    if name.is_empty() {
        return None;
    }
    let args = object
        .get("args")
        .or_else(|| object.get("arguments"))
        .or_else(|| object.get("parameters"))
        .cloned()
        .filter(|args| !args.is_null())
        .unwrap_or_else(|| serde_json::json!({}));
    if !args.is_object() {
        return None;
    }
    Some((
        ToolCall {
            name: name.to_owned(),
            args,
        },
        consumed,
        enveloped,
    ))
}

/// Parse the action object at the start of `original`, applying the JSON
/// repairs, and return how many bytes it occupies in `original` itself.
/// Repairs change lengths (an escaped newline is two bytes where the model
/// wrote one), so offsets from the repaired copy must never slice the reply:
/// doing so cut a multi-line file write past its end and crashed the run.
fn parse_action_object_in(original: &str) -> Option<(ToolCall, usize, bool)> {
    let repaired = repair_json_strings(original);
    let (call, consumed, enveloped) = parse_action_object(&repaired)?;
    let consumed = match repaired {
        std::borrow::Cow::Borrowed(_) => consumed,
        std::borrow::Cow::Owned(_) => json_value_end(original)?,
    };
    Some((call, consumed, enveloped))
}

/// Byte offset just past the first top-level JSON object or array in
/// `text`, matching brackets outside strings. It tolerates exactly the slips
/// the repairs fix (raw newlines in strings, unquoted formulas, trailing
/// commas), none of which change bracket structure.
fn json_value_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' if depth > 0 => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + ch.len_utf8());
                }
            }
            c if depth == 0 && !c.is_whitespace() => return None,
            _ => {}
        }
    }
    None
}

/// Argument names that may arrive as raw text instead of a JSON string.
/// Only the long ones: a path or a regex costs nothing to escape correctly.
const RAW_ARGS: &[&str] = &["CONTENT", "OLD", "NEW"];

/// Read raw argument blocks that follow the action object:
///
/// ```text
/// <<<CONTENT
/// the exact file text, no escaping of any kind
/// CONTENT>>>
/// ```
///
/// A file body written this way never passes through JSON string escaping,
/// which is where long writes were lost: in a real run one stray character
/// inside 10 KB of JavaScript failed the parse at column 4221 and discarded
/// the entire 12,000-character action, and the model's shorter replacement
/// was broken in a way that cost five more steps.
///
/// Returns the bytes consumed (0 when there are none), or `None` when a block
/// was opened and not terminated. `None` must keep the action unreadable: the
/// streaming loop stops as soon as an action is complete, so a block still
/// arriving cannot be allowed to look finished.
fn attach_raw_args(call: &mut ToolCall, tail: &str) -> Option<usize> {
    let mut consumed = 0usize;
    loop {
        let rest = &tail[consumed..];
        let lead = rest.len() - rest.trim_start().len();
        let after_space = &rest[lead..];
        let Some(open) = after_space.strip_prefix("<<<") else {
            return Some(consumed);
        };
        let Some(name_line) = open.split_inclusive('\n').next() else {
            return None; // the marker line itself is still arriving
        };
        let name = name_line.trim_end();
        if !RAW_ARGS.contains(&name) {
            return if consumed == 0 { Some(0) } else { None };
        }
        if !name_line.ends_with('\n') {
            return None;
        }
        let body_start = consumed + lead + 3 + name_line.len();
        let terminator = format!("{name}>>>");
        let mut offset = body_start;
        let body_end = loop {
            let Some(line) = tail[offset..].split_inclusive('\n').next() else {
                return None; // no terminator yet
            };
            if line.trim_end() == terminator {
                break offset;
            }
            offset += line.len();
            if offset >= tail.len() {
                return None;
            }
        };
        let text = &tail[body_start..body_end];
        let key = name.to_ascii_lowercase();
        match call.args.as_object_mut() {
            Some(args) => {
                args.insert(key, serde_json::Value::String(text.to_owned()));
            }
            None => return None,
        }
        // Stop at the end of the terminator text and leave its newline: a
        // closing fence must be preceded by one to stand on its own line.
        let line = tail[body_end..].split_inclusive('\n').next().unwrap_or("");
        consumed = body_end + line.trim_end().len();
    }
}

/// The action object plus any raw argument blocks after it.
fn parse_action_with_raw(body: &str) -> Option<(ToolCall, usize, bool)> {
    let (mut call, consumed, enveloped) = parse_action_object_in(body)?;
    let extra = attach_raw_args(&mut call, &body[consumed..])?;
    Some((call, consumed + extra, enveloped))
}

/// JSON string escapes, decoded leniently: an escape JSON does not define
/// keeps its backslash instead of failing, because the file wanted it.
fn lenient_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(ch @ ('"' | '\\' | '/')) => out.push(ch),
            Some('u') => {
                let hex: String = chars.clone().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => {
                        out.push(decoded);
                        for _ in 0..4 {
                            chars.next();
                        }
                    }
                    None => out.push_str("\\u"),
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Recover a complete-but-unparseable action whose damage is inside one long
/// string value. The model wrote the whole thing and closed the envelope; a
/// single bad character in the middle is no reason to discard the rest.
///
/// Only the last long argument is recovered, and only by anchoring on the
/// envelope's own end, so the value's interior is never interpreted: whatever
/// lies between the opening quote and the object's final quote is the file.
/// Everything else in the object must still parse, and the tool must be a
/// registered one, so this widens no permission and invents no arguments.
///
/// Never used while a reply is still streaming: a truncated action would be
/// "recovered" as a half file.
pub fn salvage_action(text: &str) -> Option<ToolCall> {
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    let json = text.get(start..end)?;
    let (key, at) = RAW_ARGS
        .iter()
        .filter_map(|name| {
            let key = name.to_ascii_lowercase();
            let pattern = format!("{QUOTE}{key}{QUOTE}:{QUOTE}", QUOTE = '"');
            json.rfind(&pattern).map(|at| (key, at + pattern.len()))
        })
        .max_by_key(|(_, at)| *at)?;
    // The value ends at the last quote with nothing but closing brackets
    // after it. That is the envelope's own shape, not the string's contents.
    let closing = json[at..]
        .char_indices()
        .rev()
        .find(|(index, ch)| {
            *ch == '"'
                && json[at + index + 1..]
                    .trim()
                    .chars()
                    .all(|rest| matches!(rest, '}' | ']'))
        })
        .map(|(index, _)| at + index)?;
    let value = lenient_unescape(&json[at..closing]);
    if value.trim().is_empty() {
        return None;
    }
    // The same object with that value emptied has to parse on its own.
    let skeleton = format!("{}{}", &json[..at], &json[closing..]);
    let (mut call, consumed, _) = parse_action_object_in(&skeleton)?;
    if !known_tool(&call.name) || skeleton[consumed..].trim() != "" {
        return None;
    }
    call.args.as_object_mut()?.insert(key, value.into());
    Some(call)
}

fn has_action_marker(text: &str) -> bool {
    text.contains("```") || text.contains("<tool_call>") || text.contains("<|tool_call>")
}

/// What may follow a complete action object: nothing (the model ended its
/// turn), or its closing marker and then plain prose. Returns the byte length
/// of the accepted closing, or `None` when the marker is missing. A further
/// fence or tool marker after it means more than one action, which is
/// ambiguous and rejected. A closing fence must stand on its own line.
fn action_closing(tail: &str, marker: &str) -> Result<Option<usize>, ()> {
    let Some(start) = tail.find(|ch: char| !ch.is_whitespace()) else {
        return Ok(None);
    };
    let rest = &tail[start..];
    let Some(after) = rest.strip_prefix(marker) else {
        // No closing marker at all: the object is complete, so what follows
        // is the model's narrative ("The file is ready…") unless it holds
        // another action. Small models skip the fence and keep talking.
        return if has_action_marker(rest) { Err(()) } else { Ok(None) };
    };
    if marker == "```" {
        if !tail[..start].contains('\n') {
            return Err(());
        }
        let line_rest = after.split_inclusive('\n').next().unwrap_or("");
        if !line_rest.trim().is_empty() {
            return Err(());
        }
    }
    if has_action_marker(after) {
        return Err(());
    }
    Ok(Some(start + marker.len()))
}

/// Accept one complete action fence, optionally surrounded by a progress note
/// and ordinary code blocks. Only a `tool`-labelled fence may name an unknown
/// tool (the runner then lists the known ones); `json` and bare fences count
/// as actions only when they name a registered tool, so an illustrative JSON
/// snippet in an answer is never executed. Two actions are ambiguous: never
/// select one while silently dropping another.
fn locate_fenced_action(text: &str) -> Option<LocatedAction> {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let line_start = offset;
        offset += line.len();
        let Some(action_label) = fence_opener(line) else {
            continue;
        };
        let labelled_tool = line.trim().eq_ignore_ascii_case("```tool");
        let body_start = offset;
        let body = &text[body_start..];
        let candidate = if action_label {
            parse_action_with_raw(body)
                .filter(|(call, _, enveloped)| labelled_tool || *enveloped || known_tool(&call.name))
                .map(|(call, consumed, _)| (call, consumed))
        } else {
            None
        };
        let Some((call, consumed)) = candidate else {
            // An ordinary block: skip to its closing fence and keep scanning.
            if labelled_tool {
                return None;
            }
            for inner in lines.by_ref() {
                offset += inner.len();
                if inner.trim() == "```" {
                    break;
                }
            }
            continue;
        };
        return match action_closing(&body[consumed..], "```") {
            Ok(Some(closing_len)) => Some(LocatedAction {
                call,
                span: line_start..body_start + consumed + closing_len,
                missing_close: None,
            }),
            Ok(None) => Some(LocatedAction {
                call,
                span: line_start..body_start + consumed,
                missing_close: Some("\n```"),
            }),
            Err(()) => None,
        };
    }
    None
}

/// The `<tool_call>{...}</tool_call>` envelope Qwen- and Hermes-family models
/// were trained on; they fall back to it under pressure even when the prompt
/// documents the fenced form.
fn locate_tagged_action(text: &str) -> Option<LocatedAction> {
    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let start = text.find(OPEN)?;
    if text[..start].contains("<|tool_call>") {
        return None;
    }
    let body_start = start + OPEN.len();
    let body = &text[body_start..];
    let (call, consumed, _) = parse_action_with_raw(body)?;
    match action_closing(&body[consumed..], CLOSE) {
        Ok(Some(closing_len)) => Some(LocatedAction {
            call,
            span: start..body_start + consumed + closing_len,
            missing_close: None,
        }),
        Ok(None) => Some(LocatedAction {
            call,
            span: start..body_start + consumed,
            missing_close: Some(CLOSE),
        }),
        Err(()) => None,
    }
}

/// Normalize the native text envelope observed from the installed Gemma model.
/// It must be one entire call, with JSON object arguments and no trailing text.
fn locate_gemma_action(text: &str) -> Option<LocatedAction> {
    let trimmed = text.trim();
    let body = trimmed.strip_prefix("<|tool_call>call:")?;
    let (name, body) = body.split_once("{args:")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    let body = repair_json_strings(body);
    let body: &str = &body;
    let mut values = serde_json::Deserializer::from_str(body).into_iter::<serde_json::Value>();
    let args = values.next()?.ok()?;
    if !args.is_object() || body[values.byte_offset()..].trim() != "}<tool_call|>" {
        return None;
    }
    Some(LocatedAction {
        call: ToolCall {
            name: name.to_owned(),
            args,
        },
        span: 0..text.len(),
        missing_close: None,
    })
}

/// One action in any of the shapes a local model produces: a structured
/// envelope or bare tool object, the Gemma envelope, the `<tool_call>` tag,
/// or a fenced block. This only parses: registry, mode, permission and
/// argument checks still follow.
fn locate_action(text: &str) -> Option<LocatedAction> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let (call, consumed, enveloped) = parse_action_with_raw(trimmed)?;
        if !enveloped && !known_tool(&call.name) {
            return None;
        }
        if !trimmed[consumed..].trim().is_empty() {
            return None;
        }
        return Some(LocatedAction {
            call,
            span: 0..text.len(),
            missing_close: None,
        });
    }
    if trimmed.starts_with("<|tool_call>") {
        return locate_gemma_action(text);
    }
    if text.contains("<tool_call>") {
        return locate_tagged_action(text);
    }
    locate_fenced_action(text)
}

/// Fenced-action entry point kept for the chat inspection path.
pub fn parse_tool_block(text: &str) -> Option<ToolCall> {
    locate_action(text).map(|located| located.call)
}

fn parse_action_response(text: &str) -> Option<ToolCall> {
    parse_tool_block(text)
}

/// True once the streamed text holds one complete action: the runtime can be
/// released immediately instead of generating a closing fence and prose after
/// it. Checked per token, so it must stay cheap on text without an action.
pub fn action_complete(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("<|tool_call>") {
        return text.trim_end().ends_with("<tool_call|>") && locate_gemma_action(text).is_some();
    }
    (trimmed.starts_with('{') || has_action_marker(text)) && locate_action(text).is_some()
}

/// Why an attempted action could not be read, in the JSON parser's words,
/// so the correction names the actual mistake (an unquoted formula, a
/// trailing comma) instead of restating the format. `None` when the text
/// holds no attempt or the attempt parses.
pub fn action_problem(text: &str) -> Option<String> {
    if !looks_like_action_attempt(text) || locate_action(text).is_some() {
        return None;
    }
    let mut offset = 0;
    let mut body_start = None;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if fence_opener(line) == Some(true) {
            body_start = Some(offset);
            break;
        }
    }
    let body_start = body_start.or_else(|| {
        text.find("<tool_call>").map(|index| index + "<tool_call>".len())
    })?;
    let body = &text[body_start..];
    let end = body
        .find("\n```")
        .or_else(|| body.find("</tool_call>"))
        .unwrap_or(body.len());
    let repaired = repair_json_strings(&body[..end]);
    match serde_json::from_str::<serde_json::Value>(repaired.trim()) {
        Ok(value) => {
            if value.get("name").and_then(|name| name.as_str()).is_none() {
                Some("the object has no \"name\" field".into())
            } else {
                Some("the arguments are not a JSON object".into())
            }
        }
        Err(error) => Some(error.to_string()),
    }
}

/// Did the model try to produce an action, even if none could be read? Such
/// a reply is retried as unreadable rather than reviewed as a final answer.
pub fn looks_like_action_attempt(text: &str) -> bool {
    if text.contains("```tool") || text.contains("<tool_call>") || text.contains("<|tool_call>") {
        return true;
    }
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if fence_opener(line) == Some(true) {
            let body = text[offset..].trim_start();
            if body.starts_with('{') && body.contains("\"name\"") {
                return true;
            }
        }
    }
    false
}

/// The reply as it should appear in the transcript: an unterminated action
/// gets its closing marker so later turns imitate a well-formed example.
/// Bodies below this stay verbatim: a small edit is cheap to keep, and full
/// fidelity is worth more than the tokens.
const CONDENSE_BODY_CHARS: usize = 1_500;

/// The transcript copy of a reply that carried a file body, with the body
/// replaced by a note once the host has confirmed the bytes on disk.
///
/// The reply used to be stored whole, so a 9,000-character page occupied a
/// fifth of the usable room on a 16K window and stayed there until the next
/// compaction. Keeping it buys nothing: the file is on disk, the host read it
/// back, and the tool result below says so. The progress note, the tool name
/// and the path all remain, so the model can still see what it did and read
/// the file if it needs the text again.
///
/// `None` leaves the reply untouched.
fn condensed_reply(reply: &str, call: &ToolCall, verified: bool) -> Option<String> {
    if !verified || !matches!(call.name.as_str(), "write_file" | "append_file" | "edit_file") {
        return None;
    }
    let located = locate_action(reply)?;
    let body_chars: usize = RAW_ARGS
        .iter()
        .filter_map(|name| call.args.get(name.to_ascii_lowercase().as_str()))
        .filter_map(|value| value.as_str())
        .map(|value| value.chars().count())
        .sum();
    if body_chars < CONDENSE_BODY_CHARS {
        return None;
    }
    let path = arg_str(call, "path").unwrap_or("the file");
    let note = visible_progress(reply);
    let mut out = String::new();
    if !note.trim().is_empty() {
        out.push_str(note.trim());
        out.push('\n');
    }
    out.push_str(&format!(
        "[{} {path}: {body_chars} characters, written and verified on disk. The text is not repeated here; read the file if it is needed again.]",
        call.name
    ));
    // Only worth it if it actually frees room.
    (out.chars().count() + 200 < reply[located.span.clone()].chars().count()).then_some(out)
}

fn transcript_reply(reply: &str) -> String {
    match locate_action(reply) {
        Some(LocatedAction {
            span,
            missing_close: Some(close),
            ..
        }) => {
            let mut fixed = String::with_capacity(reply.len() + close.len());
            fixed.push_str(&reply[..span.end]);
            fixed.push_str(close);
            fixed.push_str(&reply[span.end..]);
            fixed
        }
        _ => reply.to_owned(),
    }
}

/// Return the model's user-visible progress note without its machine-readable
/// action payload. This is deliberately a concise activity summary, not hidden
/// chain-of-thought.
pub fn visible_progress(text: &str) -> String {
    let mut out = String::new();
    match locate_action(text) {
        Some(located) => {
            out.push_str(&text[..located.span.start]);
            out.push_str(&text[located.span.end..]);
        }
        None => out.push_str(text),
    }
    out.trim().chars().take(1200).collect()
}

/// Output room kept free when fitting history into the window. The action
/// output cap (4K, or 8K with thinking off) is as large as a whole small
/// window, and reserving all of it left history no room at all: on a 4K
/// window every step saw only the rules, the task and its latest exchange,
/// so the model forgot finished work and repeated it. A quarter of the
/// window (at least 1,024 tokens) covers a typical action; a request still
/// uses whatever room is free.
/// Characters one reply can hold, for the model to plan against. Three per
/// token is deliberately short of the four the transcript estimate uses:
/// code is denser than prose, and a model that splits one part too early
/// loses a step, where one that overruns loses the whole action.
fn reply_char_budget(n_ctx: u32, output_cap: u32) -> usize {
    let tokens = output_cap.min(n_ctx / 3).max(256);
    tokens as usize * 3
}

fn pruning_reserve(n_ctx: u32, output_cap: u32) -> u32 {
    output_cap.min((n_ctx / 4).max(1024)).min(n_ctx / 2)
}

/// Tokens the transcript may occupy: the window minus the reply's reserve and
/// a margin for the character-based estimate.
fn history_room(n_ctx: u32, reserve: u32) -> u32 {
    n_ctx.saturating_sub(reserve).saturating_sub(512)
}

fn estimated_tokens(turns: &[ChatTurn]) -> u32 {
    crate::agent::AgentContextUsage::for_turns(turns, 0, 0, 0).estimated_tokens
}

/// Automatic compaction of a run's working transcript.
struct CompactionPolicy {
    enabled: bool,
    threshold_pct: u32,
}

impl CompactionPolicy {
    fn from_settings(memory: &crate::settings::MemorySettings) -> Self {
        Self {
            enabled: memory.auto_compaction(),
            threshold_pct: memory.compact_threshold_pct(),
        }
    }

    /// The usage percentage when compaction is due: the transcript fills the
    /// threshold share of the room history may occupy and there are earlier
    /// turns to summarize. `after_last` is the size the last compaction left:
    /// until the transcript has grown by a sixth of the room since then, a
    /// new compaction could only re-summarize the summary. On a 4K window,
    /// where the instructions alone fill almost half the room, compacting
    /// every step saved a few hundred tokens each time and cost a model call.
    fn due(
        &self,
        transcript: &[ChatTurn],
        task_turn_index: usize,
        room: u32,
        after_last: Option<u32>,
    ) -> Option<u32> {
        if !self.enabled || room == 0 {
            return None;
        }
        let used = estimated_tokens(transcript);
        let pct = (u64::from(used) * 100 / u64::from(room)) as u32;
        let compactable = task_turn_index > 1 || transcript.len() > task_turn_index + 2;
        let grown = after_last.is_none_or(|after| used >= after.saturating_add(room / 6) || pct > 100);
        (pct >= self.threshold_pct && compactable && grown).then_some(pct)
    }
}

/// Characters of `lines`, newest last, that fit `budget` characters; the
/// count of older lines left out comes first.
fn recent_lines_within(lines: &[String], budget: usize) -> (usize, Vec<&String>) {
    let mut used = 0usize;
    let mut kept: Vec<&String> = Vec::new();
    for line in lines.iter().rev() {
        used += line.len() + 3;
        if used > budget && !kept.is_empty() {
            break;
        }
        kept.push(line);
    }
    kept.reverse();
    (lines.len() - kept.len(), kept)
}

/// How host messages that relay a completion check begin; the check itself
/// is never shown them (see `review_completion`).
const REVIEW_FINDING_PREFIX: &str = "Completion review found remaining work:";
const REVIEW_UNAVAILABLE_PREFIX: &str = "The completion check could not confirm the result";

const COMPACTION_INSTRUCTION: &str ="Pause the task for a moment. The earlier turns of this conversation are about to be removed to fit your context window, and a note you write now will replace them. Write that note for yourself: what you have done so far (name every file you created or changed and every command you ran with its result), what you found or decided, what is still missing, and your next step. At most 150 words in plain sentences or bullets. Do not call tools and do not continue the task in this reply.";

struct CompactionOutcome {
    summarized_turns: usize,
    tokens_before: u32,
    tokens_after: u32,
    model_note: bool,
}

/// One line per completed state-changing action, for the record kept across
/// compaction. Reads, listings and searches are left to the progress note.
fn work_log_entry(call: &ToolCall, output: &str, failed: bool) -> Option<String> {
    let path = arg_str(call, "path").unwrap_or("");
    let first_line = |text: &str| -> String {
        text.lines()
            .find(|line| !line.trim().is_empty() && !line.starts_with("(tool"))
            .unwrap_or("")
            .chars()
            .take(160)
            .collect()
    };
    let entry = match (call.name.as_str(), failed) {
        ("execute_command", _) => {
            let command: String = arg_str(call, "command").unwrap_or("").chars().take(120).collect();
            let cwd = arg_str(call, "cwd").unwrap_or(".");
            match command_exit_code(output) {
                Some(code) => format!("ran `{command}` in {cwd}: exit code {code}"),
                None => format!("ran `{command}` in {cwd}: {}", first_line(output)),
            }
        }
        ("write_file" | "append_file", false) => first_line(output),
        ("edit_file", false) => format!("edited {path}"),
        ("delete_file", false) => format!("deleted {path}"),
        ("create_document", false) => format!(
            "saved document {} to the Artifacts panel",
            arg_str(call, "filename").unwrap_or("")
        ),
        ("git_commit", false) => format!("committed: {}", arg_str(call, "message").unwrap_or("")),
        (
            "write_file" | "append_file" | "edit_file" | "delete_file" | "create_document"
            | "git_commit",
            true,
        ) => {
            format!("{} {path} failed: {}", call.name, first_line(output))
        }
        _ => return None,
    };
    (!entry.trim().is_empty()).then_some(entry)
}

/// Summarize the earlier turns of a run into the task turn, between steps.
/// The model writes a progress note from the transcript it already has in
/// its cache; the host adds the exact record of completed actions and what
/// completion still requires, so a weak or failed note loses nothing that
/// matters. The latest exchange stays verbatim when it is small, because the
/// next step often depends on it (a file just read).
#[allow(clippy::too_many_arguments)]
async fn compact_run_transcript(
    client: &SidecarClient,
    cfg: &crate::inference::InferenceConfig,
    transcript: &mut Vec<ChatTurn>,
    task_turn_index: &mut usize,
    original_task: &str,
    work_log: &[String],
    still_required: &[String],
    room: u32,
) -> CompactionOutcome {
    let tokens_before = estimated_tokens(transcript);
    let unchanged = CompactionOutcome {
        summarized_turns: 0,
        tokens_before,
        tokens_after: tokens_before,
        model_note: false,
    };
    // The note and the record together stay within about a quarter of the
    // room, so compaction lands well under the threshold on any window.
    let note_tokens = (room / 10).clamp(96, 400);
    let record_chars = (room as usize / 8 * 4).max(600);
    // What survives any compaction: the instructions and the original task.
    let fixed = estimated_tokens(&[
        transcript[0].clone(),
        ChatTurn::text("user", original_task),
    ]);
    let expected_block = note_tokens + (record_chars / 4) as u32 + 120;
    let tail_start = transcript.len().saturating_sub(2).max(*task_turn_index + 1);
    // Keep the latest exchange (the next step often depends on it, such as a
    // file just read) only while the result still lands under 70% of room.
    let keep_tail = transcript.len() >= *task_turn_index + 3
        && transcript[tail_start].role == "assistant"
        && fixed + expected_block + estimated_tokens(&transcript[tail_start..]) <= room * 7 / 10;
    let summarize_end = if keep_tail { tail_start } else { transcript.len() };
    let summarized_turns =
        (*task_turn_index - 1) + summarize_end.saturating_sub(*task_turn_index + 1);
    if summarized_turns == 0 {
        return unchanged;
    }

    let mut request = transcript.clone();
    request.push(ChatTurn::text("user", COMPACTION_INSTRUCTION));
    let input = estimated_tokens(&request);
    let max_tokens = note_tokens.min(cfg.n_ctx.saturating_sub(input.saturating_add(256)));
    let note = if max_tokens >= 96 {
        match client.chat_turns_without_reasoning(&request, max_tokens, cfg).await {
            // A small model may still emit an action: keep only its prose.
            Ok((text, _)) => Some(visible_progress(&text)).filter(|note| !note.trim().is_empty()),
            Err(error) => {
                tracing::warn!("compaction note request failed: {error}; using the host record only");
                None
            }
        }
    } else {
        None
    };

    let mut block = format!(
        "[Context compacted between steps: {summarized_turns} earlier turn(s) of this conversation were replaced by this note to fit the model's context window. Files on disk and completed actions are unchanged.]"
    );
    if let Some(note) = &note {
        let note: String = note.trim().chars().take(note_tokens as usize * 5).collect();
        block.push_str(&format!("\nProgress note written before compaction:\n{note}"));
    }
    if !work_log.is_empty() {
        block.push_str("\nActions completed in this run (recorded by the host):");
        let (skipped, recent) = recent_lines_within(work_log, record_chars);
        if skipped > 0 {
            block.push_str(&format!("\n- ({skipped} older actions not listed)"));
        }
        for entry in recent {
            block.push_str(&format!("\n- {entry}"));
        }
    }
    for requirement in still_required {
        let requirement: String = requirement.chars().take(400).collect();
        block.push_str(&format!("\n{requirement}"));
    }
    block.push_str("\nContinue the task from here and do not redo completed actions.");

    let kept_tail: Vec<ChatTurn> = transcript[summarize_end..].to_vec();
    let mut compacted = vec![transcript[0].clone()];
    let mut task = transcript[*task_turn_index].clone();
    task.content = format!("{original_task}\n\n{block}");
    compacted.push(task);
    compacted.extend(kept_tail);
    let tokens_after = estimated_tokens(&compacted);
    // A compaction that frees less than a tenth of the room is not worth the
    // lost detail: keep the transcript as it was and let pruning act if the
    // window is actually full.
    if tokens_after.saturating_add(room / 10) > tokens_before {
        return CompactionOutcome {
            model_note: note.is_some(),
            ..unchanged
        };
    }
    *transcript = compacted;
    *task_turn_index = 1;
    CompactionOutcome {
        summarized_turns,
        tokens_before,
        tokens_after,
        model_note: note.is_some(),
    }
}

/// Keep the next request inside the context window. Old tool results are the
/// bulk of an agent transcript, so their bodies are released first (the call
/// line and a note remain; the journal keeps the full text). Only then are
/// whole turns dropped, oldest first: saved history before the task, then the
/// oldest exchanges after it. The system prompt, the task and the latest
/// exchange are never touched. Returns the number of turns condensed or removed.
/// With automatic compaction on this is only the safety net behind it.
fn prune_transcript(
    transcript: &mut Vec<ChatTurn>,
    task_turn_index: &mut usize,
    n_ctx: u32,
    output_reserve: u32,
) -> u32 {
    let budget = n_ctx.saturating_sub(output_reserve).saturating_sub(512);
    let estimate = |turns: &[ChatTurn]| {
        crate::agent::AgentContextUsage::for_turns(turns, n_ctx, 0, 0).estimated_tokens
    };
    let mut pruned = 0;
    let mut index = *task_turn_index + 1;
    while estimate(transcript) > budget && index + 2 < transcript.len() {
        let turn = &mut transcript[index];
        if turn.role == "user"
            && turn.content.starts_with("Result of ")
            && !turn.content.contains(RELEASED_MARKER)
            && turn.content.len() > 400
        {
            let head = turn
                .content
                .lines()
                .next()
                .unwrap_or("Result of tool:")
                .to_string();
            let chars = turn.content.chars().count();
            turn.content = format!(
                "{head}\n{RELEASED_MARKER} {chars} characters were released from working context to stay within the model's window. The full output is in the activity journal; run the tool again if you need it.]"
            );
            pruned += 1;
        }
        index += 1;
    }
    while estimate(transcript) > budget && *task_turn_index > 1 {
        transcript.remove(1);
        *task_turn_index -= 1;
        pruned += 1;
    }
    while estimate(transcript) > budget && transcript.len() > *task_turn_index + 3 {
        transcript.remove(*task_turn_index + 1);
        pruned += 1;
    }
    while transcript.len() > TRANSCRIPT_TURNS + 2 {
        if *task_turn_index > 1 {
            transcript.remove(1);
            *task_turn_index -= 1;
        } else if transcript.len() > *task_turn_index + 3 {
            transcript.remove(*task_turn_index + 1);
        } else {
            break;
        }
        pruned += 1;
    }
    pruned
}

/// Controlled system prompt (§22): capabilities, tool rules, cwd, OS.
pub fn system_prompt(
    workspace: &str,
    tools: &[crate::tools::ToolDescriptor],
    search_enabled: bool,
    documents_enabled: bool,
) -> String {
    system_prompt_for(
        workspace,
        tools,
        search_enabled,
        documents_enabled,
        false,
        reply_char_budget(8192, ActionResponsePolicy::default().output_cap()),
    )
}

/// Shell commands whose purpose is to open a window (a file in its default
/// app, a browser, a folder in Explorer). A model ran `start index.html` to
/// "verify animations" once open_path was withheld.
pub fn launches_window(command: &str) -> bool {
    let command = command.trim().to_lowercase();
    let first = command.split_whitespace().next().unwrap_or("");
    matches!(
        first,
        "start" | "explorer" | "explorer.exe" | "xdg-open" | "open" | "invoke-item" | "ii"
            | "start-process" | "saps" | "gio" | "wslview" | "rundll32"
    ) || command.starts_with("cmd /c start")
        || command.starts_with("cmd.exe /c start")
        || command.contains("start-process ")
        || command.starts_with("powershell start")
        || command.starts_with("pwsh start")
}

/// Whether the task asks for something to be opened for the user.
pub fn opening_requested(task: &str) -> bool {
    let task = task.to_lowercase();
    ["open ", "open it", "open the", "launch", "preview", "show it in", "in my browser", "in the browser", "in a browser", "reveal"]
        .iter()
        .any(|phrase| task.contains(phrase))
}

/// The shell `execute_command` runs a command through, named so the model
/// writes commands that exist. A model reached for `ls -la` and `echo '---'`
/// on Windows because the prompt named the operating system and not the
/// shell, and lost two steps of a run discovering it.
fn command_shell() -> &'static str {
    match std::env::consts::OS {
        "windows" => "cmd.exe (`cmd /C`), so use its commands: dir, type, copy, del, `&` between commands",
        _ => "sh (`sh -c`)",
    }
}

pub fn system_prompt_for(
    workspace: &str,
    tools: &[crate::tools::ToolDescriptor],
    search_enabled: bool,
    documents_enabled: bool,
    opening_enabled: bool,
    reply_chars: usize,
) -> String {
    let mut tool_docs = String::new();
    for t in tools {
        if t.name == "web_search" && !search_enabled {
            continue; // never advertise what the run may not use (§117)
        }
        // Documents go to the chat's Artifacts panel, not the project. Offered
        // to a coding task, a model used it to "create a project" as a zip and
        // to park the site's copy in a text file instead of the page.
        if t.name == "create_document" && !documents_enabled {
            continue;
        }
        // Opening files pops windows over the user's desktop and tells the
        // model nothing: it called open_path to "verify animations in the
        // browser". Offered only when the user asked for something opened.
        if t.name == "open_path" && !opening_enabled {
            continue;
        }
        tool_docs.push_str(&format!("- {} ({:?}): {}\n", t.name, t.risk, t.description));
        let args = match t.name {
            "list_directory" => r#"{"path":"."}"#,
            "read_file" => r#"{"path":"relative/path","start_line":1,"end_line":200}"#,
            "delete_file" | "open_path" => r#"{"path":"relative/path"}"#,
            "write_file" => r#"{"path":"relative/file"} (the file text follows in a <<<CONTENT block, rule 2b)"#,
            "append_file" => r#"{"path":"relative/file"} (the added text follows in a <<<CONTENT block, rule 2b)"#,
            "edit_file" => {
                r#"{"path":"relative/file"} (old and new follow in <<<OLD and <<<NEW blocks, rule 2b)"#
            }
            "search_text" => r#"{"query":"regex pattern","path":"."}"#,
            "execute_command" => r#"{"command":"command text","cwd":".","timeout_secs":60}"#,
            "web_search" => r#"{"query":"search terms"}"#,
            "git_commit" => r#"{"message":"commit message"}"#,
            "system_info" | "list_processes" => "{}",
            "create_document" => {
                r#"{"filename":"report.xlsx","sheets":[{"name":"Data","rows":[["Item","Amount"],["Example",1]]}]} (spec by file type: xlsx = sheets[{name,rows}]; docx and pdf = title + paragraphs[]; pptx = title + slides[{title,bullets[]}]; md/txt = text; csv = rows; html = title + html; json = data. Put the complete content in the spec. The file lands in the conversation's Artifacts panel, not in the workspace)"#
            }
            _ => "see tool description",
        };
        // Keep the JSON example clean; explanations go on their own line so a
        // model never copies prose into its argument object.
        let (json, note) = match args.split_once(" (") {
            Some((json, note)) => (json, Some(note.trim_end_matches(')'))),
            None => (args, None),
        };
        tool_docs.push_str(&format!("  args: {json}\n"));
        if let Some(note) = note {
            tool_docs.push_str(&format!("  note: {note}\n"));
        }
    }
    format!(
        "You are a local coding assistant operating inside one workspace.\n\
         Operating system: {os}\nCommands run through: {shell}\nWorkspace root: {workspace}\n\
         All `path` arguments are relative to the workspace root and cannot escape it.\n\
         Available tools:\n{tool_docs}\n\
         Rules:\n\
         1. The user's current request defines the task and its scope. Agent capability is not an instruction to change anything. Questions, explanations, reviews and diagnoses authorize inspection only; modify files only when the user requested a change. Acknowledgments and greetings never authorize new work or a different project. Never ask for slash-command syntax.\n\
         2. Use one complete action envelope with valid JSON, exactly as in this read_file example:\n\
         ```tool\n\
         {{\"name\":\"read_file\",\"args\":{{\"path\":\"README.md\"}}}}\n\
         ```\n\
         Substitute the actual tool and its arguments. Fence markers must be on separate lines. Never escape JSON object delimiters or repeat fence markers.\n\
         2b. Never put file text in JSON. Omit content, old and new from args and put each one raw after the object, between markers:\n\
         ```tool\n\
         {{\"name\":\"write_file\",\"args\":{{\"path\":\"src/app.js\"}}}}\n\
         <<<CONTENT\n\
         const pattern = /\\d+/;\n\
         CONTENT>>>\n\
         ```\n\
         Between the markers the text is written exactly as it stands, so quotes, backslashes and backticks need no escaping: this is the reliable way to write code. edit_file takes <<<OLD ... OLD>>> then <<<NEW ... NEW>>>. Each marker stands alone on its line.\n\
         2c. One response holds about {reply_chars} characters. Write a longer file in parts: write_file for the first, append_file for each next. Never shorten or simplify the work to fit, or leave a file half-written.\n\
         3. Prefer search_text before reading; read before editing; make small targeted changes with edit_file and use write_file for new files.\n\
         4. For requested implementation work, run relevant builds/tests with execute_command and iterate on failures. Do not claim that reading a file is a passing test.\n\
         5. Never invent file contents you have not read; never redo a failed identical call.\n\
         6. For build, fix, or change requests, use the tools and complete the work; do not stop at a plan or paste code for the user to apply.\n\
         7. Permission prompts are handled by the host. Do not ask how to proceed when a relevant tool can advance the task.\n\
         8. After changing files, inspect the resulting project and run the most relevant tests, build, or validation available before finishing. Opening a file or a browser shows you nothing: check by reading files, listing folders or running the project's tests.\n\
         9. Each turn must EITHER emit exactly one tool call OR, only when the task is done or impossible, give the final summary with NO tool block.\n\
         10. Conversation history defines follow-up references such as 'this project', 'those tasks', and 'it'. If the workspace contains multiple sibling projects and history identifies one of them, confine searches and reads to that project. Do not inspect unrelated sibling projects merely because they are present.\n",
        os = std::env::consts::OS,
        shell = command_shell(),
        workspace = workspace,
        tool_docs = tool_docs,
        reply_chars = reply_chars,
    )
}

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "CMakeLists.txt",
    "go.mod",
    "Makefile",
    "build.gradle",
    "pom.xml",
    "*.sln",
];

fn looks_like_project_root(path: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|marker| {
        if *marker == "*.sln" {
            return std::fs::read_dir(path)
                .map(|entries| {
                    entries.flatten().any(|entry| {
                        entry
                            .path()
                            .extension()
                            .map(|extension| extension.eq_ignore_ascii_case("sln"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
        }
        path.join(marker).exists()
    })
}

fn normalized_reference(value: &str) -> String {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A broad folder such as `Code/` may contain many independent projects. For
/// follow-ups like "this project", bind tools to the most recently named child
/// project instead of exposing unrelated siblings to retrieval or the model.
pub(crate) fn focused_workspace_root(root: &Path, conversation: &[String]) -> PathBuf {
    if looks_like_project_root(root) {
        return root.to_path_buf();
    }
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(_) => return root.to_path_buf(),
    };
    let mut candidates = std::fs::read_dir(&canonical_root)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !path.is_dir() || !looks_like_project_root(&path) {
                        return None;
                    }
                    let path = std::fs::canonicalize(path).ok()?;
                    if !path.starts_with(&canonical_root) {
                        return None;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    Some((normalized_reference(&name), path))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    for message in conversation.iter().rev() {
        let message = normalized_reference(message);
        let matches = candidates
            .iter()
            .filter(|(name, _)| {
                !name.is_empty() && format!(" {message} ").contains(&format!(" {name} "))
            })
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return matches[0].1.clone();
        }
        if matches.len() > 1 {
            return root.to_path_buf();
        }
    }
    root.to_path_buf()
}

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub workspace: PathBuf,
    pub task: String,
    pub mode: AgentMode,
    pub conversation_id: String,
    /// Stage 11: Search-toggle consent for this run (§117). web_search is
    /// only offered to the model when true.
    pub search_enabled: bool,
    /// Native thinking allowed for this run's action requests.
    pub reasoning: bool,
}

#[derive(Debug)]
pub enum ApprovalDecision {
    Approved { session: bool },
    Denied,
}

pub struct LiveRun {
    pub id: String,
    /// Stable wall-clock start, shared with the persisted assistant journal.
    pub started_at: String,
    pub spec: AgentSpec,
    pub cancel: CancelToken,
    pub events: Mutex<Vec<AgentEvent>>,
    pub broadcaster: broadcast::Sender<AgentEvent>,
    /// Ordered journal writer, separate from lossy live broadcast subscribers.
    pub activity_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    pub pending: Mutex<Option<PendingTool>>,
    pub pending_tx: Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
    pub handle: Mutex<Option<JoinHandle<AgentState>>>,
}

impl LiveRun {
    pub fn emit(&self, ev: AgentEvent) {
        let _ = self.activity_tx.send(ev.clone());
        self.events.lock().expect("lock").push(ev.clone());
        let _ = self.broadcaster.send(ev);
    }

    /// Transient progress for live subscribers only: streamed partial model
    /// text. Never journaled and never replayed; the completed thought is
    /// journaled as one event when the response is validated.
    pub fn emit_live(&self, ev: AgentEvent) {
        let _ = self.broadcaster.send(ev);
    }

    pub fn state(&self) -> AgentState {
        self.events
            .lock()
            .expect("lock")
            .last()
            .map(|e| e.state)
            .unwrap_or(AgentState::Idle)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub id: String,
    pub started_at: String,
    pub task: String,
    pub state: AgentState,
    pub iterations: u32,
    pub conversation_id: String,
}

#[derive(Default)]
pub struct AgentRegistry {
    runs: HashMap<String, Arc<LiveRun>>,
    order: VecDeque<String>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, run: Arc<LiveRun>) {
        self.order.push_back(run.id.clone());
        if self.order.len() > 20 {
            if let Some(old) = self.order.pop_front() {
                self.runs.remove(&old);
            }
        }
        self.runs.insert(run.id.clone(), run);
    }

    pub fn get(&self, id: &str) -> Option<Arc<LiveRun>> {
        self.runs.get(id).cloned()
    }

    pub fn summaries(&self) -> Vec<RunSummary> {
        self.order
            .iter()
            .filter_map(|id| self.runs.get(id))
            .map(|r| {
                let evs = r.events.lock().expect("lock");
                RunSummary {
                    id: r.id.clone(),
                    started_at: r.started_at.clone(),
                    task: r.task_short(),
                    state: evs.last().map(|e| e.state).unwrap_or(AgentState::Idle),
                    iterations: evs.last().map(|e| e.iteration).unwrap_or(0),
                    conversation_id: r.spec.conversation_id.clone(),
                }
            })
            .collect()
    }
}

impl LiveRun {
    fn task_short(&self) -> String {
        self.spec.task.chars().take(80).collect()
    }
}

/// The loop. Owns no locks across awaits except short clones.
pub async fn run_loop(state: crate::api::AppState, run: Arc<LiveRun>) -> AgentState {
    use AgentState as S;
    let spec = run.spec.clone();
    let conversation_scope = if spec.conversation_id.is_empty() {
        Vec::new()
    } else {
        state
            .storage
            .lock()
            .await
            .context_messages_for(&spec.conversation_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content)
            .collect()
    };
    let focused_root = focused_workspace_root(&spec.workspace, &conversation_scope);
    let ws = crate::workspace::WorkspaceManager::new(focused_root.clone());
    let ws_key = focused_root.to_string_lossy().into_owned();
    let limits = AgentLimits {
        max_iterations: state.settings.read().await.agent.max_iterations,
        ..AgentLimits::default()
    };

    // Snapshot the sidecar; without inference there is no reasoning engine (§4).
    let sidecar = {
        let mut llama = state.llama.write().await;
        if llama.is_running() {
            llama
                .running
                .as_ref()
                .map(|r| (r.base_url.clone(), r.cfg.clone()))
        } else {
            None
        }
    };
    let Some((base_url, cfg)) = sidecar else {
        run.emit(AgentEvent::new(
            S::Failed,
            "No inference running. Start inference first, then retry the task.".into(),
            0,
        ));
        return S::Failed;
    };
    let client = match SidecarClient::new(base_url) {
        Ok(c) => c,
        Err(e) => {
            run.emit(AgentEvent::new(
                S::Failed,
                format!("Sidecar client failed: {e}"),
                0,
            ));
            return S::Failed;
        }
    };

    run.emit(AgentEvent::activity(
        "task",
        S::Planning,
        spec.task.clone(),
        0,
    ));
    if focused_root != spec.workspace {
        run.emit(AgentEvent::activity(
            "status",
            S::Planning,
            format!(
                "Focused this follow-up on {} from the conversation; unrelated sibling projects are excluded.",
                focused_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("the referenced project")
            ),
            0,
        ));
    }
    let objective = match spec.mode {
        AgentMode::Plan => format!("Inspect the project using read-only tools and produce a practical implementation plan for this request. Do not implement changes or run commands. Request: {}", spec.task),
        AgentMode::CodeAssist => format!("Inspect the project using read-only tools and answer this exact request from evidence in the project. This is an inspection, not a request for an implementation plan. Resolve follow-up references from the preceding conversation and do not inspect unrelated sibling projects. Request: {}", spec.task),
        _ => spec.task.clone(),
    };
    let documents_enabled = crate::documents::requested_document_kind(&spec.task).is_some();
    let opening_enabled = opening_requested(&spec.task);
    let mut prompt = system_prompt_for(
        &ws_key,
        &crate::tools::registry(),
        spec.search_enabled,
        documents_enabled,
        opening_enabled,
        reply_char_budget(cfg.n_ctx, ActionResponsePolicy::default().output_cap()),
    );
    if matches!(spec.mode, AgentMode::Plan | AgentMode::CodeAssist) {
        prompt.push_str("\nThis run is READ ONLY. Only safe inspection tools are permitted. Do not write, edit, delete, create documents, execute commands or change Git state. Finish with findings or a plan, not implementation.");
    }
    let mut transcript = vec![ChatTurn::text("system", prompt)];
    // Each run starts fresh inference, not fresh product memory. Carry the
    // session's derived context and attachment excerpts into follow-up tasks.
    if !spec.conversation_id.is_empty() {
        let st = state.storage.lock().await;
        let workspace = st
            .get_conversation(&spec.conversation_id)
            .ok()
            .flatten()
            .map(|conversation| conversation.workspace)
            .unwrap_or_default();
        if let Ok(memory) = st.memory_context(&spec.conversation_id, &workspace) {
            transcript[0].content.push_str(&memory.text);
        }
        let mut history = st
            .context_messages_for(&spec.conversation_id)
            .unwrap_or_default();
        history.retain(|message| message.id != run.id);
        if history
            .last()
            .map(|message| message.role == "user" && message.content.trim() == spec.task.trim())
            .unwrap_or(false)
        {
            history.pop();
        }
        let attachments = st
            .attachments_for(&spec.conversation_id)
            .unwrap_or_default();
        let budget = crate::api::history_char_budget(cfg.n_ctx, transcript[0].content.len());
        transcript.extend(crate::api::build_turns_budgeted(
            &history,
            &attachments,
            budget,
        ));
    }
    transcript.push(ChatTurn::text(
            "user",
            format!(
                "Task: {}\n\nAddress this request within its scope. Use the complete tool envelope shown in the system instructions if an action is necessary; otherwise answer directly. Do not invent a new task or mutate files for a question.",
                objective
            ),
        ));
    let mut task_turn_index = transcript.len() - 1;
    let original_task = transcript[task_turn_index].content.clone();
    let compaction = CompactionPolicy::from_settings(&state.settings.read().await.memory);
    let compaction_pct = if compaction.enabled { compaction.threshold_pct } else { 0 };
    let mut work_log: Vec<String> = Vec::new();
    let mut compactions = 0u32;
    let mut tokens_after_last_compaction: Option<u32> = None;
    // Six consecutive non-successful steps end the run. Each failure carries a
    // specific correction (received args, closest matching text), and a model
    // often needs two or three of them to converge; identical loops and the
    // iteration limit are bounded separately.
    let mut progress_guard = crate::agent_progress::ProgressGuard::new(6);
    let mut last_file: Option<String> = None;
    let mut response_policy = ActionResponsePolicy {
        disable_native_thinking: !spec.reasoning,
        ..ActionResponsePolicy::default()
    };
    let mut pending_continuation: Option<String> = None;
    let mut completion_reviews = 0u32;
    let mut last_review_reason = String::new();
    let mut repeated_review = 0u32;
    let mut repeated_despite_changes = 0u32;
    let mut actions_since_review = 0u32;
    let mut no_action_pushback_used = false;
    let mut verification = VerificationState::for_task(&spec.task);
    let mut failed_calls = FailedCalls::default();
    let mut pruned_turns = 0u32;
    let mut tool_evidence: Vec<String> = Vec::new();

    for it in 1..=limits.max_iterations {
        if run.cancel.is_cancelled() {
            run.emit(AgentEvent::new(
                S::Cancelled,
                "Cancelled by user.".into(),
                it,
            ));
            return S::Cancelled;
        }
        let reserve = pruning_reserve(cfg.n_ctx, response_policy.output_cap());
        let room = history_room(cfg.n_ctx, reserve);
        // Automatic compaction happens here and only here: between steps,
        // once the previous action and its result are recorded, with no model
        // reply, tool, approval or completion check in flight. A partial
        // reply awaiting its continuation is a step still in progress.
        if pending_continuation.is_none() {
            if let Some(pct) =
                compaction.due(&transcript, task_turn_index, room, tokens_after_last_compaction)
            {
                run.emit(AgentEvent::activity(
                    "status",
                    S::Compacting,
                    format!(
                        "Compacting context: {pct}% of the usable context is in use (automatic compaction starts at {}%). The run is paused and resumes as soon as the summary is ready…",
                        compaction.threshold_pct
                    ),
                    it,
                ));
                let mut still_required = Vec::new();
                if verification.needs_evidence() {
                    still_required.push(format!(
                        "Still required before finishing: {}",
                        verification.remaining()
                    ));
                }
                if !last_review_reason.is_empty() {
                    still_required.push(format!(
                        "The latest completion check found remaining work: {last_review_reason}"
                    ));
                }
                let outcome = compact_run_transcript(
                    &client,
                    &cfg,
                    &mut transcript,
                    &mut task_turn_index,
                    &original_task,
                    &work_log,
                    &still_required,
                    room,
                )
                .await;
                // Also after a compaction that was not worth applying: the
                // next attempt waits until the transcript has grown again.
                tokens_after_last_compaction = Some(outcome.tokens_after);
                if outcome.summarized_turns > 0 {
                    compactions += 1;
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        format!(
                            "Context compacted: {} earlier turn(s) summarized, about {} → {} tokens ({}). Resuming the task.",
                            outcome.summarized_turns,
                            outcome.tokens_before,
                            outcome.tokens_after,
                            if outcome.model_note {
                                "the model's progress note plus the host's record of completed actions"
                            } else {
                                "the host's record of completed actions; the model's note was unavailable"
                            }
                        ),
                        it,
                    ));
                    run.emit(AgentEvent::context(
                        S::Planning,
                        it,
                        crate::agent::AgentContextUsage::for_turns(
                            &transcript,
                            cfg.n_ctx,
                            reserve,
                            pruned_turns,
                        )
                        .with_compaction(compactions, room, compaction_pct),
                    ));
                } else {
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        "Compaction would not free enough context to be worth it; resuming with the current context.".into(),
                        it,
                    ));
                }
                if run.cancel.is_cancelled() {
                    run.emit(AgentEvent::new(S::Cancelled, "Cancelled by user.".into(), it));
                    return S::Cancelled;
                }
            }
        }
        // Keep the transcript inside the context window (§21): release old
        // tool-result bodies first, drop whole turns only when that is not
        // enough. The full outputs stay in the journal.
        pruned_turns += prune_transcript(&mut transcript, &mut task_turn_index, cfg.n_ctx, reserve);
        if !state.llama.write().await.is_running() {
            run.emit(AgentEvent::activity("error", S::Failed,
                "The model runtime stopped. Completed actions and existing file changes were kept. Load a model before continuing.".into(), it));
            return S::Failed;
        }
        if !progress_guard.begin_attempt() {
            let message = format!("Stopped after {} consecutive steps that produced no successful action (failed tools, unreadable or denied actions, or completion claims the check rejected). Existing files and recorded actions were kept; review the last error before retrying.", progress_guard.attempts_without_progress());
            persist_final(&state, &run, &message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message, it - 1));
            return S::Failed;
        }
        run.emit(AgentEvent::activity(
            "status",
            S::Planning,
            if tool_evidence.is_empty() {
                "Preparing the first action…".into()
            } else {
                "Reviewing the result and preparing the next action…".into()
            },
            it,
        ));
        // Partial visible text is request-local until a complete response has
        // been assembled and validated. It is never executed or journaled as
        // completed work, and never includes native reasoning text.
        let use_structured = response_policy.structured_fallback && pending_continuation.is_none();
        let mut request_turns = pending_continuation
            .as_deref()
            .map(|prefix| continuation_turns(&transcript, prefix))
            .unwrap_or_else(|| transcript.clone());
        if use_structured {
            request_turns.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        }
        let estimated_input =
            crate::agent::AgentContextUsage::for_turns(&request_turns, cfg.n_ctx, 0, pruned_turns)
                .estimated_tokens;
        let output_budget = response_policy.output_budget(cfg.n_ctx, estimated_input);
        if output_budget < 128 {
            let message = "There is not enough room in this model's context for another action. Saved history and existing files were kept. Compact the conversation or use a larger context before retrying.";
            persist_final(&state, &run, message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message.into(), it));
            return S::Failed;
        }
        let input_context = crate::agent::AgentContextUsage::for_turns(
            &request_turns,
            cfg.n_ctx,
            output_budget,
            pruned_turns,
        )
        .with_compaction(compactions, room, compaction_pct);
        run.emit(AgentEvent::context(S::Planning, it, input_context.clone()));
        // Stream the action: partial text reaches live subscribers as it is
        // produced, and a complete action releases the runtime immediately.
        // A suffix continuation cannot be parsed on its own, and a schema-
        // constrained reply ends itself, so neither stops early.
        let stop_on_action = pending_continuation.is_none() && !use_structured;
        let live_run = run.clone();
        let live_cancel = run.cancel.clone();
        let handlers = StreamHandlers {
            on_token: Box::new(move |delta| {
                live_run.emit_live(AgentEvent::activity(
                    "thought_delta",
                    S::Planning,
                    delta.to_string(),
                    it,
                ));
            }),
            is_cancelled: Box::new(move || live_cancel.is_cancelled()),
            should_stop: Box::new(move |text| stop_on_action && action_complete(text)),
            ..StreamHandlers::default()
        };
        let mut completion = match client
            .agent_chat_turns_stream(
                &request_turns,
                output_budget,
                &cfg,
                response_policy.disable_native_thinking,
                use_structured,
                handlers,
            )
            .await
        {
            Ok(completion) => {
                run.emit(AgentEvent::context(
                    S::Planning,
                    it,
                    input_context.reported(
                        completion.metrics.prompt_tokens,
                        completion.metrics.generated_tokens,
                    ),
                ));
                completion
            }
            Err(e) => {
                run.emit(AgentEvent::activity(
                    "status",
                    S::Planning,
                    format!(
                        "The model request failed: {e}. Attempt {} of 4 without progress.",
                        progress_guard.attempts_without_progress()
                    ),
                    it,
                ));
                transcript.push(ChatTurn::text(
                    "user",
                    format!("The model call failed ({e}). Adjust and continue, or finish with a summary."),
                ));
                continue;
            }
        };
        let continuation_failed = if let Some(prefix) = pending_continuation.take() {
            match assemble_continuation(&prefix, &completion.text) {
                Some(combined) => {
                    completion.text = combined;
                    false
                }
                None => true,
            }
        } else {
            false
        };
        let reply = completion.text.clone();
        let mut parsed_call = parse_action_response(&reply);
        // A finished action whose JSON broke inside one long value is
        // recovered rather than thrown away. In a real run a 12,084-character
        // file write failed to parse at column 4221 and was discarded whole;
        // the model's shorter replacement was broken in a way that cost five
        // further steps. Never attempted on a cut-off reply, which would
        // "recover" half a file.
        if parsed_call.is_none()
            && !continuation_failed
            && !completion.is_truncated()
            && looks_like_action_attempt(&reply)
        {
            if let Some(call) = salvage_action(&reply) {
                run.emit(AgentEvent::activity(
                    "status",
                    S::Planning,
                    format!(
                        "The action's JSON was malformed, but it was complete: recovered the {} call and its {} characters of content by reading the envelope's own boundaries. {}",
                        call.name,
                        call.args
                            .as_object()
                            .and_then(|args| RAW_ARGS
                                .iter()
                                .filter_map(|name| args.get(name.to_ascii_lowercase().as_str()))
                                .filter_map(|value| value.as_str())
                                .map(|value| value.chars().count())
                                .max())
                            .unwrap_or(0),
                        action_problem(&reply)
                            .map(|problem| format!("The parser had reported: {problem}."))
                            .unwrap_or_default(),
                    ),
                    it,
                ));
                parsed_call = Some(call);
            }
        }
        let invalid_reason = if continuation_failed {
            Some("The continuation returned no usable extension of the partial response.".into())
        } else {
            action_response_invalid_reason(&completion, parsed_call.as_ref(), output_budget)
        };
        if let Some(reason) = invalid_reason {
            if can_continue_response(&completion) && response_policy.begin_continuation() {
                let continuation_input = crate::agent::AgentContextUsage::for_turns(
                    &continuation_turns(&transcript, &reply),
                    cfg.n_ctx,
                    0,
                    pruned_turns,
                )
                .estimated_tokens;
                let retry_budget = response_policy.output_budget(cfg.n_ctx, continuation_input);
                pending_continuation = Some(reply);
                run.emit(AgentEvent::activity("status", S::Planning,
                    format!("{reason} Continuing from the saved partial response once, with native thinking disabled and up to {retry_budget} more output tokens. The assembled action must validate before anything executes."), it));
                continue;
            }
            if !response_policy.recover_invalid() {
                let message = format!("{reason} Both controlled retries also failed, so I stopped instead of repeating them. No partial or unreadable action was executed. Existing work was kept.");
                persist_final(&state, &run, &message).await;
                run.emit(AgentEvent::activity("error", S::Failed, message, it));
                return S::Failed;
            }
            let retry_budget = response_policy.output_budget(cfg.n_ctx, estimated_input);
            let strategy = if response_policy.structured_fallback {
                "Retrying with the schema-constrained JSON envelope"
            } else {
                "Retrying with a format reminder and native thinking disabled for this run"
            };
            // Journal what could not be read, why, and how it ended, so the
            // failure is diagnosable from the activity view. The head alone
            // hid the cause of a rejected file write in a real run.
            let trimmed = reply.trim();
            let total_chars = trimmed.chars().count();
            let head: String = trimmed.chars().take(900).collect();
            let tail: String = if total_chars > 1300 {
                let skip = total_chars - 400;
                format!("\n[… {} characters …]\n{}", skip - 900, trimmed.chars().skip(skip).collect::<String>())
            } else {
                trimmed.chars().skip(900).collect()
            };
            let problem = action_problem(&reply);
            run.emit(AgentEvent::activity("status", S::Planning,
                format!("{reason} {strategy}, with up to {retry_budget} output tokens. Incomplete actions are discarded.{}{}",
                    problem.as_ref().map(|problem| format!("\nWhy it could not be read: {problem}")).unwrap_or_default(),
                    if head.is_empty() { String::new() } else { format!("\n\nUnreadable response ({total_chars} characters):\n{head}{tail}") }), it));
            // Say what actually went wrong. Asking for shorter content after a
            // mere escaping slip made a model replace a real animation script
            // with a one-line alert; that advice is for a cut-off reply only.
            let cut_off = completion.is_truncated();
            let mut correction = String::from(if cut_off {
                "Your previous response was cut off by the output limit; no action from it was executed."
            } else {
                "Your previous response was empty or could not be read; no action from it was executed."
            });
            if let Some(problem) = &problem {
                correction.push_str(&format!(" The JSON parser reported: {problem}. Inside a JSON string a backslash may only precede \" \\ / b f n r t or u; write ' and ` without a backslash."));
            }
            correction.push_str(" Return exactly one complete tool envelope as shown in the system example, with a name and object args and its closing fence; outer JSON keys use ordinary quotes.");
            if cut_off {
                correction.push_str(" Keep this action smaller: write one file at a time, or split a long file into a first part and an edit that adds the rest.");
            } else {
                correction.push_str(" Send the same action again with its complete content, corrected; do not shorten or simplify the content because of the error.");
            }
            correction.push_str(" If the work is finished, return a brief final answer instead. Output one action only.");
            transcript.push(ChatTurn::text("user", correction));
            continue;
        }
        // Failed/truncated payloads never pollute the subsequent transcript.
        transcript.push(ChatTurn::text("assistant", transcript_reply(&reply)));

        let Some(call) = parsed_call else {
            let reply = parse_structured_reply(&reply)
                .filter(|envelope| envelope.kind == "final")
                .map(|envelope| envelope.answer)
                .unwrap_or(reply);
            // Journal the claim itself: when the check below rejects it, the
            // activity view must show what the model said, not only that it
            // was sent back to work.
            let claim: String = reply.trim().chars().take(1200).collect();
            if !claim.is_empty() {
                run.emit(AgentEvent::activity("thought", S::Planning, claim, it));
            }
            // Read-only runs deliver findings/plans, not implemented code.
            // A second model critic cannot verify an implementation here and
            // can trap small local models in repeated format-correction loops.
            if matches!(spec.mode, AgentMode::Plan | AgentMode::CodeAssist) {
                persist_final(&state, &run, &reply).await;
                run.emit(AgentEvent::activity("final", S::Completed, reply, it));
                return S::Completed;
            }
            // A change request answered without a single action is not done.
            // A 14B model replied "I am unable to create projects" to a
            // request to build a website, and its own completion check
            // accepted that as an honest explanation. Once per run, say what
            // it can do and send it back to work; a second tool-free answer
            // goes to the normal check.
            if tool_evidence.is_empty() && !no_action_pushback_used {
                no_action_pushback_used = true;
                run.emit(AgentEvent::activity(
                    "status",
                    S::Planning,
                    "No action has been taken yet for this change request. Reminding the model which tools it has and continuing…".into(),
                    it,
                ));
                transcript.push(ChatTurn::text(
                    "user",
                    "No action has been taken in this run, and this request asks for work in the workspace. You can create folders and files with write_file (a new path creates its folders), change files with edit_file, and run commands with execute_command. Start the work now with one tool call. Only if something specific truly blocks it, name that blocker instead.",
                ));
                continue;
            }
            // A tool-free answer is only a candidate completion. Coding models,
            // especially smaller local ones, often summarize after producing a
            // subset of the requested files. Check their claim against the
            // original task and the actual tool evidence before ending the run.
            run.emit(AgentEvent::activity(
                "status",
                S::Observing,
                "Checking the work against your request…".into(),
                it,
            ));

            let review = if verification.needs_evidence() {
                CompletionReview::Continue(verification.remaining())
            } else {
                review_completion(&client, &cfg, &transcript, &objective, &tool_evidence).await
            };

            match review {
                CompletionReview::Complete => {
                    persist_final(&state, &run, &reply).await;
                    run.emit(AgentEvent::activity("final", S::Completed, reply, it));
                    return S::Completed;
                }
                CompletionReview::Continue(reason) => {
                    completion_reviews += 1;
                    let reason = reason.trim().chars().take(600).collect::<String>();
                    // The same finding again means one of two things. With no
                    // change in between, the model cannot act on it: stop with
                    // the finding named. With changes in between, the checker
                    // is the one repeating itself (it once quoted a missing
                    // hype line three times while the page already had one):
                    // finish, and leave the finding in the answer for the user.
                    if reason == last_review_reason {
                        if actions_since_review == 0 {
                            repeated_review += 1;
                        } else {
                            repeated_despite_changes += 1;
                        }
                    } else {
                        repeated_review = 0;
                        repeated_despite_changes = 0;
                        last_review_reason = reason.clone();
                    }
                    actions_since_review = 0;
                    if repeated_review >= 2 {
                        let final_message = format!(
                            "I produced the same result three times without addressing what the check found, so I stopped. Remaining work: {reason}. Existing files were kept."
                        );
                        persist_final(&state, &run, &final_message).await;
                        run.emit(AgentEvent::activity("error", S::Failed, final_message, it));
                        return S::Failed;
                    }
                    if repeated_despite_changes >= 2 {
                        let final_message = format!(
                            "{}\n\nNote: my completion check kept reporting \"{reason}\" even after the files were changed to address it, so it may be mistaken. Please review the result.",
                            reply.trim()
                        );
                        persist_final(&state, &run, &final_message).await;
                        run.emit(AgentEvent::activity("final", S::Completed, final_message, it));
                        return S::Completed;
                    }
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        format!("Found remaining work. Continuing: {reason}"),
                        it,
                    ));
                    transcript.push(ChatTurn::text(
                        "user",
                        format!(
                            "Completion review found remaining work: {reason}\nContinue executing now. Do not summarize again until you have addressed this and verified the result."
                        ),
                    ));
                    if completion_reviews > limits.max_retries_per_tool + 2 {
                        let final_message = format!(
                            "I reviewed the partial work but could not finish it reliably. Remaining work: {reason}"
                        );
                        persist_final(&state, &run, &final_message).await;
                        run.emit(AgentEvent::activity("error", S::Failed, final_message, it));
                        return S::Failed;
                    }
                    continue;
                }
                CompletionReview::Unavailable(reason) => {
                    completion_reviews += 1;
                    transcript.push(ChatTurn::text(
                        "user",
                        format!(
                            "The completion check could not confirm the result ({reason}). Inspect the workspace, verify every requested deliverable, then continue or finish with evidence."
                        ),
                    ));
                    run.emit(AgentEvent::activity(
                        "status",
                        S::Planning,
                        "Completion was not verified. Inspecting the project and continuing…"
                            .into(),
                        it,
                    ));
                    if completion_reviews > limits.max_retries_per_tool {
                        let final_message = "I could not verify that the work is complete because the local model stopped responding reliably. The partial work has been kept.";
                        persist_final(&state, &run, final_message).await;
                        run.emit(AgentEvent::activity(
                            "error",
                            S::Failed,
                            final_message.into(),
                            it,
                        ));
                        return S::Failed;
                    }
                    continue;
                }
            }
        };
        let progress = visible_progress(&reply);
        if !progress.is_empty() {
            run.emit(AgentEvent::activity("thought", S::Planning, progress, it));
        }

        // Validate the call before any gate or execution (§93).
        if !crate::tools::registry().iter().any(|t| t.name == call.name) {
            let known: Vec<&str> = crate::tools::registry().iter().map(|t| t.name).collect();
            transcript.push(ChatTurn::text(
                "user",
                format!(
                    "Unknown tool '{}'. Available: {}. Reply with a valid call or a summary.",
                    call.name,
                    known.join(", ")
                ),
            ));
            run.emit(AgentEvent::new(
                S::Observing,
                format!("Unknown tool '{}'; asked model to correct.", call.name),
                it,
            ));
            continue;
        }
        let unavailable = match call.name.as_str() {
            "create_document" if !documents_enabled => Some("create_document is not available for this task. It saves a standalone document (report, spreadsheet, slides) to the chat's Artifacts panel, outside the project. Create project files, including any text the page should show, with write_file."),
            "open_path" if !opening_enabled => Some("open_path is not available for this task: the user did not ask for anything to be opened, and opening a file shows you nothing. Check your work by reading files, listing folders or running the project's tests or build."),
            "execute_command"
                if !opening_enabled && launches_window(arg_str(&call, "command").unwrap_or("")) =>
            {
                Some("This command opens a window on the user's desktop (a browser, Explorer or another app), and the user did not ask for anything to be opened; it was not run. Opening something shows you nothing. Check your work by reading files, listing folders or running the project's tests or build.")
            }
            _ => None,
        };
        if let Some(message) = unavailable {
            // The reply itself is already in the transcript; add the refusal.
            transcript.push(ChatTurn::text("user", format!("Result of {}:\n(tool error, do not retry identically)\n{message}", call.name)));
            run.emit(AgentEvent::tool_activity(
                "tool_error",
                S::Observing,
                format!("Action failed: {}", call.name),
                it,
                call.name.clone(),
                call.args.clone(),
                Some(message.into()),
                None,
            ));
            let _ = progress_guard.observe_tool_result(&call.name, &call.args, message, false);
            continue;
        }
        let risk = crate::tools::risk_of(&call.name);
        if !spec.mode.allows_risk(risk) {
            transcript.push(ChatTurn::text(
                "user",
                format!("'{call}' is disabled in this read-only run. Use reads/search and finish with a plan or findings; the user must explicitly switch to Agent to execute changes.", call = call.name),
            ));
            run.emit(AgentEvent::new(
                S::Observing,
                format!("Blocked {} (read-only mode).", call.name),
                it,
            ));
            continue;
        }

        // --- Gate (§92) ---
        // Agent mode is already folded in: CodeAssist blocks writes above,
        // Chat is refused at spawn; the shared PermissionManager decides.
        // Drop the policy read lock before awaiting a user decision. Otherwise
        // switching Ask → Auto cannot acquire the write lock to release it.
        let decision =
            state
                .permissions
                .read()
                .await
                .decide_in(&call.name, risk, true, Some(&ws_key));
        let approved = match decision {
            PermissionDecision::Allow => true,
            PermissionDecision::Deny { reason } => {
                transcript.push(ChatTurn::text(
                    "user",
                    format!("Denied: {reason}. Work around it or finish."),
                ));
                run.emit(AgentEvent::new(
                    S::Observing,
                    format!("Denied {}: {reason}", call.name),
                    it,
                ));
                continue;
            }
            PermissionDecision::RequireApproval { reason } => {
                match await_approval(&state, &run, &call, &reason, it).await {
                    Some(ApprovalDecision::Approved { session }) => {
                        if session && risk == RiskLevel::Moderate {
                            state
                                .permissions
                                .write()
                                .await
                                .grant_session(&call.name, &ws_key);
                        }
                        true
                    }
                    Some(ApprovalDecision::Denied) | None => {
                        transcript.push(ChatTurn::text(
                            "user",
                            format!(
                                "The user denied '{}'. Adjust the plan or finish with a summary.",
                                call.name
                            ),
                        ));
                        run.emit(AgentEvent::new(
                            S::Observing,
                            format!("User denied {}.", call.name),
                            it,
                        ));
                        continue;
                    }
                }
            }
        };

        // --- Execute ---
        let before = mutation_snapshot(&ws, &call);
        run.emit(AgentEvent::tool_activity(
            "tool_started",
            S::ExecutingTool,
            tool_action_label(&call, false),
            it,
            call.name.clone(),
            call.args.clone(),
            None,
            None,
        ));
        // Stage 20: documents render through the async pipeline (audited).
        let output = if call.name == "create_document" {
            let conv = if run.spec.conversation_id.trim().is_empty() {
                format!("agent:{}", run.id)
            } else {
                run.spec.conversation_id.clone()
            };
            match crate::documents::execute_create_document(
                &state.storage,
                &state.artifacts_dir,
                &conv,
                &call.args,
            )
            .await
            {
                Ok(r) => {
                    audit_tool(&state, &run, &call, &r.output).await;
                    r.output
                }
                Err(e) => format!("(document failed: {e})"),
            }
        } else if call.name == "web_search" {
            if !spec.search_enabled {
                "(web search is OFF for this run — enable Search to use it)".to_string()
            } else {
                // Search opt-in is separate from approvals. Auto never prompts
                // for enabled Search, but an explicit deny still blocks it.
                let settings = state.settings.read().await.clone();
                let global_auto = state.permissions.read().await.autonomy
                    == crate::permissions::AutonomyLevel::Autonomous;
                match settings.search.autonomous.as_str() {
                    "deny" => "(web search is disabled by policy for autonomous runs)".to_string(),
                    _ if global_auto => execute_web_search(&state, &run, &call).await,
                    _ => {
                        match await_approval(
                            &state,
                            &run,
                            &call,
                            "Web access leaves the machine (query + retrieved pages).",
                            it,
                        )
                        .await
                        {
                            Some(ApprovalDecision::Approved { .. }) => {
                                execute_web_search(&state, &run, &call).await
                            }
                            _ => "(the user denied web search — continue from local knowledge)"
                                .to_string(),
                        }
                    }
                }
            }
        } else {
            let mut output = execute_local_tool(&state, &run, &ws, &call, approved).await;
            // A missing path is the most common argument slip; the file the
            // model was just working on is almost always the one it meant.
            if output.starts_with("(tool error")
                && output.contains("requires {\"path\"")
                && arg_str(&call, "path").is_none()
            {
                if let Some(file) = &last_file {
                    output.push_str(&format!(
                        "\nHint: you did not send a path. The file you last worked on was {file}."
                    ));
                }
            }
            output
        };
        if let Some(path) = arg_str(&call, "path").filter(|path| !path.trim().is_empty()) {
            if matches!(
                call.name.as_str(),
                "read_file" | "edit_file" | "write_file" | "append_file" | "search_text"
            ) {
                last_file = Some(path.to_string());
            }
        }
        let shown: String = output.chars().take(tool_output_chars(room)).collect();
        let tool_failed = tool_output_failed(&output);
        if let Some(entry) = work_log_entry(&call, &output, tool_failed) {
            work_log.push(entry);
            if !tool_failed {
                actions_since_review += 1;
            }
        }
        // Document results carry no artifact id, so an identical re-render
        // reads as an identical result and the repeat guard can see it.
        let observation =
            progress_guard.observe_tool_result(&call.name, &call.args, &output, !tool_failed);
        if observation.is_progress() {
            response_policy.useful_progress();
            completion_reviews = 0;
        }
        // A schema-constrained request that produced an invalid action (for
        // example empty args) is not helping; return to the native format.
        if tool_failed
            && use_structured
            && output.contains("Invalid tool arguments")
        {
            response_policy.structured_fallback = false;
            run.emit(AgentEvent::activity("status", S::Planning,
                "The constrained JSON action format produced invalid arguments; switching back to the native action format for the next step.".into(), it));
        }
        verification.observe(&ws, &call, &output, tool_failed);
        let repeated_failure = failed_calls.observe(&call, tool_failed);
        tool_evidence.push(tool_evidence_line(&call, &shown));
        if tool_evidence.len() > 20 {
            tool_evidence.remove(0);
        }
        if !tool_failed && output.contains(crate::tools::WRITE_VERIFIED) {
            if let Some(condensed) = condensed_reply(&reply, &call, true) {
                if let Some(turn) = transcript
                    .iter_mut()
                    .rev()
                    .find(|turn| turn.role == "assistant")
                {
                    turn.content = condensed;
                }
            }
        }
        transcript.push(ChatTurn::text(
            "user",
            format!("Result of {}:\n{}", call.name, shown),
        ));
        run.emit(AgentEvent::tool_activity(
            if tool_failed {
                "tool_error"
            } else {
                "tool_result"
            },
            S::Observing,
            if tool_failed {
                format!("Action failed: {}", call.name)
            } else {
                tool_action_label(&call, true)
            },
            it,
            call.name.clone(),
            call.args.clone(),
            Some(shown),
            if tool_failed {
                None
            } else {
                mutation_diff(&ws, &call, before)
            },
        ));
        if repeated_failure {
            let message = format!("I stopped after the same {} action failed three times without a correction. Existing changes were kept. Review the recorded error and choose a different approach before continuing.", call.name);
            persist_final(&state, &run, &message).await;
            run.emit(AgentEvent::activity("error", S::Failed, message, it));
            return S::Failed;
        }
        if tool_failed {
            run.emit(AgentEvent::activity(
                "status",
                S::Planning,
                "That step failed. Evaluating another approach…".into(),
                it,
            ));
        }
        if let crate::agent_progress::Observation::Repeated { warn, stop } = observation {
            if stop {
                let message = format!("I stopped because the same {} action returned the identical result {} times in a row; the run was looping rather than progressing. Completed changes were kept. Review the recorded actions and give a more specific next step.", call.name, progress_guard.identical_repeats() + 1);
                persist_final(&state, &run, &message).await;
                run.emit(AgentEvent::activity("error", S::Failed, message, it));
                return S::Failed;
            }
            if warn {
                transcript.push(ChatTurn::text(
                    "user",
                    format!("[Host note: this {} result is identical to the one you already received. Do not repeat it; use the evidence you have, take a different action, or finish with a summary.]", call.name),
                ));
            }
        }
    }
    let final_message = format!(
        "I checked the partial work, but the agent reached its {}-step safety limit before the task was complete. The existing changes have been kept.",
        limits.max_iterations
    );
    persist_final(&state, &run, &final_message).await;
    run.emit(AgentEvent::activity(
        "error",
        S::Failed,
        final_message,
        limits.max_iterations,
    ));
    S::Failed
}

enum CompletionReview {
    Complete,
    Continue(String),
    Unavailable(String),
}

/// The review rides on the run's own transcript (which already ends with the
/// candidate answer) plus one instruction turn, so the cached KV prefix serves
/// everything but the instruction. A separate prompt would evict the
/// transcript from the single slot and force a full re-prefill next iteration.
async fn review_completion(
    client: &SidecarClient,
    cfg: &crate::inference::InferenceConfig,
    transcript: &[ChatTurn],
    task: &str,
    evidence: &[String],
) -> CompletionReview {
    // The check rides on the transcript, so its evidence gets only the room
    // the window has left: twenty 400-character lines on top of a 2K-token
    // transcript overflowed a 4K window and the check always failed.
    let spare_tokens = cfg
        .n_ctx
        .saturating_sub(estimated_tokens(transcript))
        .saturating_sub(220 + 256 + 400);
    let evidence_chars = (spare_tokens as usize * 3).min(8_000);
    let evidence = if evidence.is_empty() {
        "No tools were used.".to_string()
    } else {
        let (omitted, recent) = recent_lines_within(evidence, evidence_chars);
        let mut text = recent.into_iter().cloned().collect::<Vec<_>>().join("\n");
        if omitted > 0 {
            text = format!("({omitted} earlier actions omitted to fit the context window)\n{text}");
        }
        text
    };
    // The check must judge the work, not repeat its earlier verdicts: with
    // them in view it quoted the same missing item after it was fixed.
    let transcript: Vec<ChatTurn> = transcript
        .iter()
        .filter(|turn| {
            !(turn.role == "user"
                && (turn.content.starts_with(REVIEW_FINDING_PREFIX)
                    || turn.content.starts_with(REVIEW_UNAVAILABLE_PREFIX)))
        })
        .cloned()
        .collect();
    let transcript = transcript.as_slice();
    let instruction = format!(
        "Stop and act as a strict completion checker for your own work above. When a file was written or edited more than once, judge only its latest version. Compare the original task with your candidate final answer and the recorded tool evidence. Do not assume files exist unless the evidence shows them. For change requests, require all requested deliverables plus a post-change inspection, test, build, or other relevant validation. Check every explicit requirement of the task (for example each feature, section, style, interaction or animation it names) against what the written content actually contains: a placeholder, a stub or a single alert does not implement a requirement. The agent can create folders and files, edit files and run commands in this workspace, so a claim that it cannot is not an honest explanation. If the task is fully complete or genuinely impossible for a specific stated reason, reply exactly COMPLETE. Otherwise reply CONTINUE: followed by one concise description of the missing work. Reply with that decision only.\n\nOriginal task:\n{task}\n\nTool evidence:\n{evidence}"
    );
    let mut turns = transcript.to_vec();
    turns.push(ChatTurn::text("user", instruction));
    match client.chat_turns_without_reasoning(&turns, 220, cfg).await {
        Ok((answer, _)) => parse_completion_review(&answer),
        Err(error) => CompletionReview::Unavailable(error.to_string()),
    }
}

fn parse_completion_review(answer: &str) -> CompletionReview {
    let trimmed = answer.trim();
    let upper = trimmed.to_ascii_uppercase();
    if upper == "COMPLETE" || upper.starts_with("COMPLETE\n") {
        CompletionReview::Complete
    } else if upper.starts_with("CONTINUE") {
        let reason = trimmed
            .split_once(':')
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or("Inspect the current project and finish every requested deliverable.");
        CompletionReview::Continue(reason.to_string())
    } else {
        CompletionReview::Unavailable(
            "The completion checker returned no usable decision. This is not evidence that the requested work is complete."
                .into(),
        )
    }
}

/// Conservative command classification is evidence labeling, not a sandbox or
/// permission bypass. Shell execution follows the user's Ask/Auto mode.
fn simple_command_words(command: &str) -> Vec<String> {
    if command.contains([
        ';', '&', '|', '>', '<', '\n', '\r', '`', '$', '(', ')', '\'', '"',
    ]) {
        return vec![];
    }
    command
        .split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn verification_command(command: &str) -> bool {
    let words = simple_command_words(command);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["npm" | "pnpm" | "yarn" | "bun", "test" | "build" | "check" | "lint" | "typecheck", ..] => {
            true
        }
        ["npm" | "pnpm" | "yarn" | "bun", "run", "test" | "build" | "check" | "lint" | "typecheck", ..] => {
            true
        }
        ["cargo", "test" | "check" | "build" | "clippy", ..] => true,
        ["go", "test" | "vet" | "build", ..] => true,
        ["dotnet", "test" | "build", ..] => true,
        ["pytest" | "pytest.exe" | "ruff" | "mypy", ..] => true,
        ["python" | "python3" | "py", "-m", "pytest" | "unittest" | "compileall", ..] => true,
        ["node", "--test", ..] => true,
        ["node", file, ..] => {
            let file_name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            file.ends_with(".test.js")
                || file.ends_with(".test.mjs")
                || file.ends_with(".test.cjs")
                || file.ends_with(".spec.js")
                || file.ends_with(".spec.mjs")
                || file.ends_with(".spec.cjs")
                || matches!(
                    file_name,
                    "verify.js"
                        | "verify.mjs"
                        | "verify.cjs"
                        | "check.js"
                        | "check.mjs"
                        | "check.cjs"
                        | "test.js"
                        | "test.mjs"
                        | "test.cjs"
                )
        }
        _ => false,
    }
}

fn read_only_command(command: &str) -> bool {
    let words = simple_command_words(command);
    matches!(
        words.first().map(String::as_str),
        Some(
            "echo"
                | "pwd"
                | "ls"
                | "dir"
                | "type"
                | "cat"
                | "get-content"
                | "get-childitem"
                | "get-location"
        )
    ) || matches!(words.as_slice(), [git, sub, ..] if git == "git" && matches!(sub.as_str(), "status" | "diff" | "log" | "show"))
}

/// What the host must see before a completion claim is even reviewed. It asks
/// only for evidence every project can produce: a look at what changed, or
/// the project's own tests or build when it has them. An earlier version
/// demanded a recognised test or build after any shell command, which a new
/// folder, a static website or a script task never has: `mkdir` alone made
/// such runs unfinishable.
#[derive(Default)]
struct VerificationState {
    /// Files whose latest revision nothing has confirmed. A later mutation
    /// re-inserts its path: older reads cannot verify a new revision.
    uninspected: HashSet<PathBuf>,
    /// Folders a command ran in whose effects the host cannot see (scripts,
    /// package managers, generators, mkdir). Each needs one look afterwards:
    /// a listing of that folder (or a read-only listing or git command run
    /// there), or tests or a build passing from that folder or above.
    unobserved_dirs: HashSet<PathBuf>,
    /// How the model named each tracked path, for the remaining-work message.
    labels: HashMap<PathBuf, String>,
    /// The file type the task explicitly asked for (presentation, spreadsheet,
    /// PDF…): only a produced file fulfils it, never its content in the chat.
    document_kind: Option<&'static str>,
    document_created: bool,
}

impl VerificationState {
    fn for_task(task: &str) -> Self {
        Self {
            document_kind: crate::documents::requested_document_kind(task),
            ..Self::default()
        }
    }

    fn needs_evidence(&self) -> bool {
        !self.uninspected.is_empty()
            || !self.unobserved_dirs.is_empty()
            || (self.document_kind.is_some() && !self.document_created)
    }

    fn label(&self, path: &Path) -> String {
        let label = self
            .labels
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        if label == "." || label.is_empty() {
            "\".\" (the workspace root)".into()
        } else {
            format!("\"{label}\"")
        }
    }

    fn remaining(&self) -> String {
        if let (Some(kind), false) = (self.document_kind, self.document_created) {
            return format!(
                "The task asks for a {kind} file and none has been produced: writing its content into the chat is not the deliverable. Create it now with one create_document call (filename ending in .{kind}, complete content in the spec), then report the result."
            );
        }
        let mut steps = Vec::new();
        if !self.unobserved_dirs.is_empty() {
            let mut dirs: Vec<String> = self.unobserved_dirs.iter().map(|dir| self.label(dir)).collect();
            dirs.sort();
            steps.push(format!(
                "list {} with list_directory to see what your commands left there",
                dirs.join(" and ")
            ));
        }
        if !self.uninspected.is_empty() {
            let mut files: Vec<String> = self.uninspected.iter().map(|file| self.label(file)).collect();
            files.sort();
            steps.push(format!(
                "read the changed part of {} with read_file to confirm the result",
                files.join(", ")
            ));
        }
        format!(
            "Before finishing, {}. If the project has tests or a build, running them successfully also counts. Report what you saw; a listing or a read is not a passing test.",
            steps.join(", and ")
        )
    }

    /// A listing of `dir` shows what commands that ran there left behind.
    fn observe_listing(&mut self, dir: &Path) {
        self.unobserved_dirs.remove(dir);
    }

    fn observe(
        &mut self,
        ws: &crate::workspace::WorkspaceManager,
        call: &ToolCall,
        output: &str,
        failed: bool,
    ) {
        if call.name == "execute_command" {
            let command = arg_str(call, "command").unwrap_or_default();
            let cwd_label = arg_str(call, "cwd").unwrap_or(".");
            let Ok(cwd) = ws.resolve(cwd_label) else {
                return;
            };
            if verification_command(command) {
                // The full host-produced result includes its real process exit.
                // Failed validation cannot discharge earlier mutation evidence.
                if !failed && command_exit_code(output) == Some(0) {
                    self.uninspected.retain(|path| !path.starts_with(&cwd));
                    self.unobserved_dirs.retain(|dir| !dir.starts_with(&cwd));
                }
            } else if read_only_command(command) {
                if !failed {
                    if let Some(listed) = listing_target(command, cwd_label, ws) {
                        self.observe_listing(&listed);
                    }
                    if git_overview_command(command) {
                        // git status/diff report changes anywhere below.
                        self.unobserved_dirs.retain(|dir| !dir.starts_with(&cwd));
                    }
                }
            } else {
                // A command may partially mutate files even with nonzero exit.
                self.labels.entry(cwd.clone()).or_insert_with(|| cwd_label.to_string());
                self.unobserved_dirs.insert(cwd);
            }
            return;
        }
        if failed {
            return;
        }
        if call.name == "create_document" {
            self.document_created = true;
            return;
        }
        let path_label = arg_str(call, "path");
        let path = path_label.and_then(|path| ws.resolve(path).ok());
        if let (Some(path), Some(label)) = (&path, path_label) {
            self.labels.entry(path.clone()).or_insert_with(|| label.to_string());
        }
        match call.name.as_str() {
            "write_file" | "append_file" | "edit_file" => {
                // A file written with the requested extension is the deliverable too.
                if let (Some(kind), Some(label)) = (self.document_kind, path_label) {
                    if label.to_ascii_lowercase().ends_with(&format!(".{kind}")) {
                        self.document_created = true;
                    }
                }
                if let Some(path) = path {
                    // A write the host read back byte for byte is confirmed;
                    // an edit's surroundings still deserve a look.
                    if call.name != "edit_file" && output.contains(crate::tools::WRITE_VERIFIED) {
                        self.uninspected.remove(&path);
                    } else {
                        self.uninspected.insert(path);
                    }
                }
            }
            "delete_file" => {
                // Host-checked absence is deletion evidence, never test evidence.
                if let Some(path) = path {
                    if !path.exists() {
                        self.uninspected.remove(&path);
                    } else {
                        self.uninspected.insert(path);
                    }
                }
            }
            "read_file" => {
                if let Some(path) = path {
                    self.uninspected.remove(&path);
                }
            }
            "list_directory" => {
                let listed = ws.resolve(path_label.unwrap_or(".")).ok();
                if let Some(listed) = listed {
                    self.observe_listing(&listed);
                }
            }
            _ => {}
        }
    }
}

/// The folder a read-only listing command shows: its working directory, or
/// the single plain path it names (`dir energy-drink`, `ls src`).
fn listing_target(
    command: &str,
    cwd_label: &str,
    ws: &crate::workspace::WorkspaceManager,
) -> Option<PathBuf> {
    let words = simple_command_words(command);
    if !matches!(
        words.first().map(String::as_str),
        Some("ls" | "dir" | "get-childitem")
    ) {
        return None;
    }
    // Same shape check as above, but paths keep their case.
    let original: Vec<&str> = command.split_whitespace().collect();
    let paths: Vec<&str> = original[1..]
        .iter()
        .copied()
        .filter(|word| !word.starts_with('-') && !word.starts_with('/'))
        .collect();
    match paths.as_slice() {
        [] => ws.resolve(cwd_label).ok(),
        [path] => ws.resolve(&format!("{cwd_label}/{path}")).ok(),
        _ => None,
    }
}

fn git_overview_command(command: &str) -> bool {
    matches!(
        simple_command_words(command).as_slice(),
        [git, sub, ..] if git == "git" && matches!(sub.as_str(), "status" | "diff")
    )
}

#[derive(Default)]
struct FailedCalls(HashMap<String, u32>);

impl FailedCalls {
    fn observe(&mut self, call: &ToolCall, failed: bool) -> bool {
        let key = format!("{}:{}", call.name, call.args);
        if !failed {
            self.0.remove(&key);
            return false;
        }
        let count = self.0.entry(key).or_default();
        *count += 1;
        *count >= 3
    }
}

fn tool_evidence_line(call: &ToolCall, output: &str) -> String {
    let target = arg_str(call, "path")
        .or_else(|| arg_str(call, "query"))
        .or_else(|| arg_str(call, "command"))
        .unwrap_or("");
    let result = output
        .replace(['\r', '\n'], " ")
        .chars()
        .take(320)
        .collect::<String>();
    let evidence_kind = match call.name.as_str() {
        "read_file" | "list_directory" | "search_text" => "INSPECTION ONLY (not a test)",
        "execute_command" if verification_command(target) => {
            "VALIDATION COMMAND (check actual exit)"
        }
        "execute_command" => "COMMAND (not verification; may mutate)",
        "write_file" | "append_file" | "edit_file" | "delete_file" => {
            "FILE CHANGE (not verification)"
        }
        _ => "ACTION",
    };
    format!(
        "- {evidence_kind}: {} [{}] => {}",
        call.name, target, result
    )
}

fn command_exit_code(output: &str) -> Option<i32> {
    output
        .strip_prefix("Command:\n")?
        .split_once("\n\nExit code:\n")?
        .1
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn tool_output_failed(output: &str) -> bool {
    output.starts_with("(tool reported failure)")
        || output.starts_with("(tool error")
        || output.starts_with("(document failed:")
        || output.starts_with("(web search failed:")
        || output.starts_with("(web search is OFF")
        || output.starts_with("(web search is disabled")
        || output.starts_with("(the user denied web search")
        || (output.starts_with("Command:\n") && command_exit_code(output) != Some(0))
}

/// Pause the loop until the user approves/denies (or the run is stopped).
async fn await_approval(
    state: &crate::api::AppState,
    run: &Arc<LiveRun>,
    call: &ToolCall,
    reason: &str,
    it: u32,
) -> Option<ApprovalDecision> {
    let (tx, rx) = oneshot::channel();
    let pending = PendingTool {
        tool: call.name.clone(),
        args: call.args.clone(),
        reason: reason.into(),
    };
    *run.pending.lock().expect("lock") = Some(pending.clone());
    *run.pending_tx.lock().expect("lock") = Some(tx);
    run.emit({
        let mut ev = AgentEvent::new(
            AgentState::WaitingPermission,
            format!("{} needs approval: {reason}", call.name),
            it,
        );
        ev.pending_tool = Some(pending);
        ev.kind = "permission".into();
        ev.tool = Some(call.name.clone());
        ev.args = Some(call.args.clone());
        ev
    });
    {
        // Publishing before taking the settings-update lock closes both race
        // orders: an Auto setter sees this pending action, or this re-check
        // sees the Auto preference that was persisted just before publication.
        // Never hold this lock (or a policy read guard) while waiting on the user.
        let _update = state.settings_update.lock().await;
        crate::api::resume_auto_approved_runs(state).await;
    }
    let decision = rx.await.ok();
    *run.pending.lock().expect("lock") = None;
    decision
}

async fn execute_local_tool(
    state: &crate::api::AppState,
    run: &LiveRun,
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
    approved: bool,
) -> String {
    let tool_req = crate::tools::ToolRequest {
        name: call.name.clone(),
        args: call.args.clone(),
        approved,
    };
    // Commands can run for minutes and searches walk whole trees: blocking
    // work belongs on the blocking pool, not on an async worker that also
    // serves other sessions' streams.
    let blocking_ws = ws.clone();
    let blocking_req = tool_req.clone();
    let executed = tokio::task::spawn_blocking(move || {
        crate::tools::execute(&blocking_req, &blocking_ws, true)
    })
    .await
    .unwrap_or_else(|error| {
        Err(crate::tools::ToolError::InvalidArgs(format!(
            "tool task failed: {error}"
        )))
    });
    let output = match executed {
        Ok(r) => {
            let mut text = r.output;
            if !r.ok {
                text = format!("(tool reported failure)\n{text}");
            }
            text
        }
        Err(e) => match e {
            crate::tools::ToolError::InvalidArgs(_) | crate::tools::ToolError::Workspace(_) => {
                // Say exactly what was received: a model that sent
                // {"file":"x"} instead of {"path":"x"} can correct itself only
                // if it sees the difference.
                format!(
                    "(tool error, do not retry identically)\n{e}\nYou sent args: {}",
                    short_args(&call.args)
                )
            }
            _ => format!("(tool error, do not retry identically)\n{e}"),
        },
    };
    audit_tool(state, run, call, &output).await;
    output
}

async fn execute_web_search(
    state: &crate::api::AppState,
    run: &LiveRun,
    call: &ToolCall,
) -> String {
    let query = call
        .args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let settings = state.settings.read().await.clone();
    let scfg = crate::search::SearchConfig {
        provider: settings.search.provider.clone(),
        brave_key: settings.search.brave_key.clone(),
        custom_url: settings.search.custom_url.clone(),
        max_results: settings.search.max_results,
        timeout_secs: settings.search.timeout_secs,
    };
    let output = match crate::search::run_search(&query, &scfg).await {
        Ok((results, provider)) => {
            let conv = if run.spec.conversation_id.trim().is_empty() {
                format!("agent:{}", run.id)
            } else {
                run.spec.conversation_id.clone()
            };
            let st = state.storage.lock().await;
            let _ = st.record_search_run(&crate::storage::SearchRun {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv.clone(),
                query: query.chars().take(500).collect(),
                provider: provider.clone(),
                result_count: results.len(),
                created_at: chrono::Utc::now().to_rfc3339(),
            });
            let mut text = format!("Web results via {provider}:\n");
            for (i, r) in results.iter().enumerate() {
                text.push_str(&format!(
                    "{}. {} ({})\n{}\n",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                ));
            }
            let _ = st.record_tool_execution(&crate::storage::ToolExecution {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv,
                tool: "web_search".into(),
                args: call.args.to_string().chars().take(4000).collect(),
                result: text.chars().take(4000).collect(),
                approved: true,
                created_at: chrono::Utc::now().to_rfc3339(),
            });
            text
        }
        Err(e) => format!(
            "(web search failed: {e}. Continue from local knowledge; never invent results.)"
        ),
    };
    output
}

fn short_args(args: &serde_json::Value) -> String {
    let s = args.to_string();
    if s.len() > 160 {
        format!("{}…", s.chars().take(160).collect::<String>())
    } else {
        s
    }
}

fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    call.args.get(key).and_then(|v| v.as_str())
}

fn tool_action_label(call: &ToolCall, completed: bool) -> String {
    let target = arg_str(call, "path")
        .or_else(|| arg_str(call, "query"))
        .or_else(|| arg_str(call, "command"))
        .unwrap_or("");
    let verb = match (call.name.as_str(), completed) {
        ("read_file", false) => "Reading file",
        ("read_file", true) => "Read file",
        ("list_directory", false) => "Listing directory",
        ("list_directory", true) => "Listed directory",
        ("search_text", false) => "Searching code",
        ("search_text", true) => "Searched code",
        ("web_search", false) => "Searching the web",
        ("web_search", true) => "Searched the web",
        ("edit_file", false) => "Modifying file",
        ("edit_file", true) => "Modified file",
        ("write_file", false) => "Writing file",
        ("write_file", true) => "Wrote file",
        ("append_file", false) => "Adding to file",
        ("append_file", true) => "Added to file",
        ("delete_file", false) => "Deleting file",
        ("delete_file", true) => "Deleted file",
        ("execute_command", false) => "Running command",
        ("execute_command", true) => "Ran command",
        ("git_commit", false) => "Creating commit",
        ("git_commit", true) => "Created commit",
        ("create_document", false) => "Creating document",
        ("create_document", true) => "Created document",
        (_, false) => "Running tool",
        (_, true) => "Finished tool",
    };
    if target.is_empty() {
        format!("{verb}: {}", call.name)
    } else {
        format!("{verb}: {target}")
    }
}

fn mutation_snapshot(
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
) -> Option<Option<String>> {
    if !["edit_file", "write_file", "append_file", "delete_file"].contains(&call.name.as_str()) {
        return None;
    }
    let path = ws.resolve(arg_str(call, "path")?).ok()?;
    if !path.exists() {
        return Some(None);
    }
    if std::fs::metadata(&path).ok()?.len() > 64_000 {
        return None;
    }
    Some(Some(std::fs::read_to_string(path).ok()?))
}

fn mutation_diff(
    ws: &crate::workspace::WorkspaceManager,
    call: &ToolCall,
    before: Option<Option<String>>,
) -> Option<String> {
    let before = before?;
    let after = mutation_snapshot(ws, call)?;
    if before == after {
        return None;
    }
    let path = arg_str(call, "path")?;
    let old = before.as_deref().unwrap_or("");
    let new = after.as_deref().unwrap_or("");
    let removed = old
        .lines()
        .map(|line| format!("-{line}\n"))
        .collect::<String>();
    let added = new
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    Some(format!(
        "--- {}\n+++ {}\n@@ -1,{} +1,{} @@\n{removed}{added}",
        if before.is_some() {
            format!("a/{path}")
        } else {
            "/dev/null".into()
        },
        if after.is_some() {
            format!("b/{path}")
        } else {
            "/dev/null".into()
        },
        old.lines().count(),
        new.lines().count()
    ))
}

async fn audit_tool(state: &crate::api::AppState, run: &LiveRun, call: &ToolCall, output: &str) {
    let conv = if run.spec.conversation_id.trim().is_empty() {
        format!("agent:{}", run.id)
    } else {
        run.spec.conversation_id.clone()
    };
    let st = state.storage.lock().await;
    let _ = st.record_tool_execution(&crate::storage::ToolExecution {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conv,
        tool: call.name.clone(),
        args: call.args.to_string().chars().take(4000).collect(),
        result: output.chars().take(4000).collect(),
        approved: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    });
}

async fn persist_final(state: &crate::api::AppState, run: &LiveRun, content: &str) {
    if run.spec.conversation_id.trim().is_empty() {
        return;
    }
    let st = state.storage.lock().await;
    if !matches!(st.get_conversation(&run.spec.conversation_id), Ok(Some(_))) {
        return;
    }
    let _ = st.update_message_content(&run.spec.conversation_id, &run.id, content);
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_verified_file_body_is_not_kept_a_second_time_in_the_transcript() {
        let body = "const value = 1;\n".repeat(200);
        let reply = format!(
            concat!(
                "Writing the page.\n```tool\n",
                r#"{{"name":"write_file","args":{{"path":"site/app.js"}}}}"#,
                "\n<<<CONTENT\n{body}CONTENT>>>\n```\n"
            ),
            body = body
        );
        let call = parse_action_response(&reply).expect("action");
        let condensed = condensed_reply(&reply, &call, true).expect("condensed");
        // The note and what was done survive; the file text does not.
        assert!(condensed.contains("Writing the page."));
        assert!(condensed.contains("site/app.js"));
        assert!(!condensed.contains("const value = 1;"));
        assert!(
            estimated_tokens(&[ChatTurn::text("assistant", condensed.clone())]) * 8
                < estimated_tokens(&[ChatTurn::text("assistant", reply.clone())]),
            "must free most of the room it occupied"
        );
        // Unverified writes and small edits are left exactly as they were.
        assert!(condensed_reply(&reply, &call, false).is_none());
        let small = concat!(
            "```tool\n",
            r#"{"name":"edit_file","args":{"path":"a.js"}}"#,
            "\n<<<OLD\nconst a = 1;\nOLD>>>\n<<<NEW\nconst a = 2;\nNEW>>>\n```"
        );
        let small_call = parse_action_response(small).expect("edit");
        assert!(condensed_reply(small, &small_call, true).is_none());
    }

    #[test]
    fn a_complete_action_with_broken_json_is_recovered_not_discarded() {
        // The shape of the real loss: a long body in a JSON string with one
        // stray unescaped quote in the middle. serde stops at that quote and
        // reports "expected `,` or `}`", and every byte after it used to go
        // in the bin along with the rest of the action.
        let body = format!(
            "const a = 1;{}const bad = \"unescaped;{}const z = 2;",
            "\\n".repeat(1),
            "\\n".repeat(1)
        );
        let reply = format!(
            "Writing it.\n<tool_call>\n{{\"name\":\"write_file\",\"args\":{{\"path\":\"app.js\",\"content\":\"{body}\"}}}}\n</tool_call>"
        );
        assert!(parse_action_response(&reply).is_none(), "must not parse strictly");
        assert!(action_problem(&reply).is_some(), "the parser states a reason");

        let call = salvage_action(&reply).expect("recovered");
        assert_eq!(call.name, "write_file");
        assert_eq!(call.args["path"], "app.js");
        let content = call.args["content"].as_str().unwrap();
        // Everything the model wrote is present, the stray quote included,
        // and the escapes it did write correctly became real characters.
        assert!(content.starts_with("const a = 1;\n"));
        assert!(content.contains("const bad = \"unescaped;"));
        assert!(content.ends_with("const z = 2;"));
    }

    #[test]
    fn salvage_never_invents_an_action_or_rescues_a_cut_off_one() {
        // No closing envelope: the reply stopped mid-string, so there is no
        // end to anchor on and half a file must not be written.
        let truncated = "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"a.js\",\"content\":\"half a fi";
        assert!(salvage_action(truncated).is_none());
        // An unregistered tool is never conjured out of malformed text.
        let unknown = "```tool\n{\"name\":\"rm_rf\",\"args\":{\"path\":\"a\",\"content\":\"x\"unquoted\"}}\n```";
        assert!(salvage_action(unknown).is_none());
        // Prose that merely mentions a tool is not an action.
        assert!(salvage_action("I would use write_file with content here.").is_none());
    }

    /// The failure this envelope exists for: a file body carrying exactly the
    /// characters JSON escaping gets wrong. Written raw, none of them matter.
    const HOSTILE_JS: &str = r#"const re = /\d+\.\d+/;
const msg = "it's \"quoted\"";
const tpl = `line ${a}`;
"#;

    #[test]
    fn a_file_body_can_arrive_raw_instead_of_as_a_json_string() {
        let reply = format!(
            concat!(
                "Writing the script.\n",
                "```tool\n",
                r#"{{"name":"write_file","args":{{"path":"app.js"}}}}"#,
                "\n<<<CONTENT\n{body}CONTENT>>>\n```\n"
            ),
            body = HOSTILE_JS,
        );
        let call = parse_action_response(&reply).expect("raw content action");
        assert_eq!(call.name, "write_file");
        assert_eq!(call.args["path"], "app.js");
        // Byte for byte, trailing newline included.
        assert_eq!(call.args["content"], HOSTILE_JS);
    }

    #[test]
    fn a_raw_block_still_arriving_is_not_a_complete_action() {
        // The stream stops the moment an action reads as complete, so a block
        // that has opened and not closed must read as unfinished. Otherwise
        // the file body is cut off mid-write.
        let opening = concat!(
            "```tool\n",
            r#"{"name":"write_file","args":{"path":"app.js"}}"#,
            "\n<<<CONTENT\nhalf a fi"
        );
        assert!(!action_complete(opening));
        assert!(parse_action_response(opening).is_none());
        let finished = format!("{opening}le\nCONTENT>>>\n```\n");
        assert!(action_complete(&finished));
        assert_eq!(
            parse_action_response(&finished).unwrap().args["content"],
            "half a file\n"
        );
    }

    #[test]
    fn an_edit_takes_both_halves_raw() {
        let reply = concat!(
            "<tool_call>\n",
            r#"{"name":"edit_file","args":{"path":"a.js"}}"#,
            "\n<<<OLD\n",
            r#"const a = "x";"#,
            "\nOLD>>>\n<<<NEW\n",
            r#"const a = "y";"#,
            "\nNEW>>>\n</tool_call>"
        );
        let call = parse_action_response(reply).expect("raw edit");
        assert_eq!(call.name, "edit_file");
        assert_eq!(call.args["old"], "const a = \"x\";\n");
        assert_eq!(call.args["new"], "const a = \"y\";\n");
    }

    #[test]
    fn json_arguments_are_unaffected_by_the_raw_form() {
        let reply = concat!(
            "```tool\n",
            r#"{"name":"write_file","args":{"path":"a.txt","content":"plain"}}"#,
            "\n```"
        );
        let call = parse_action_response(reply).expect("json action");
        assert_eq!(call.args["content"], "plain");
    }

    use super::*;

    /// Explicit opt-in diagnostic: generates text only, never executes tools.
    #[tokio::test]
    #[ignore = "requires an idle local model and COMPANION_ACTION_PROBE_URL"]
    async fn local_model_action_format_probe() {
        let url = std::env::var("COMPANION_ACTION_PROBE_URL").expect("explicit probe URL required");
        assert!(url.starts_with("http://127.0.0.1:"), "local probe only");
        let prompt = system_prompt("C:/isolated-fixture", &crate::tools::registry(), false, false);
        let task = std::env::var("COMPANION_ACTION_PROBE_TASK").unwrap_or_else(|_| "In this isolated evaluation fixture, inspect calculator.cjs and verify.cjs. Fix add(a,b) so it returns the sum, delete only disposable.txt, then execute node verify.cjs and report its actual result. Do not change verify.cjs, install packages, or use the network. Keep the change minimal.".into());
        let mut turns = vec![ChatTurn::text("system", prompt), ChatTurn::text("user", format!("Task: {task}\n\nAddress this request within its scope. Use the complete tool envelope shown in the system instructions if an action is necessary; otherwise answer directly. Do not invent a new task or mutate files for a question."))];
        let structured = std::env::var("COMPANION_ACTION_PROBE_STRUCTURED").is_ok();
        if structured {
            turns.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        }
        let reply = SidecarClient::new(url)
            .unwrap()
            .agent_chat_turns(
                &turns,
                512,
                &crate::inference::InferenceConfig::default(),
                true,
                structured,
            )
            .await
            .unwrap();
        eprintln!(
            "Visible action only: {:?}; finish: {:?}",
            reply.text, reply.finish_reason
        );
        assert!(!reply.is_truncated());
        assert!(parse_action_response(&reply.text).is_some());
    }

    async fn approval_race_fixture(state: &crate::api::AppState) -> Arc<LiveRun> {
        let (activity_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let run = Arc::new(LiveRun {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            spec: AgentSpec {
                workspace: std::env::temp_dir(),
                task: "Approval race fixture; no execution".into(),
                mode: AgentMode::Agent,
                conversation_id: String::new(),
                search_enabled: false,
                reasoning: false,
            },
            cancel: CancelToken::new(),
            events: Mutex::new(vec![]),
            broadcaster: broadcast::channel(8).0,
            activity_tx,
            pending: Mutex::new(None),
            pending_tx: Mutex::new(None),
            handle: Mutex::new(None),
        });
        state.agents.write().await.insert(run.clone());
        run
    }

    async fn save_auto_for_race_test(state: &crate::api::AppState) {
        // The test caller holds settings_update just as the real setters do.
        let mut settings = state.settings.read().await.clone();
        settings.agent.autonomous_enabled = true;
        state.storage.lock().await.save_settings(&settings).unwrap();
        *state.settings.write().await = settings;
        state.permissions.write().await.autonomy = crate::permissions::AutonomyLevel::Autonomous;
    }

    #[tokio::test]
    async fn approval_rechecks_auto_saved_before_pending_action_is_published() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        {
            let _update = state.settings_update.lock().await;
            save_auto_for_race_test(&state).await;
            assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 0);
        }
        let call = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"echo fixture"}),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            await_approval(&state, &run, &call, "Earlier Ask decision", 1),
        )
        .await
        .expect("a just-published action must see already-saved Auto");
        assert!(matches!(
            result,
            Some(ApprovalDecision::Approved { session: false })
        ));
        assert!(run.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_published_before_auto_save_is_resumed_without_lock_deadlock() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        let mut events = run.broadcaster.subscribe();
        let update = state.settings_update.lock().await;
        let bg_state = state.clone();
        let bg_run = run.clone();
        let waiter = tokio::spawn(async move {
            let call = ToolCall {
                name: "delete_file".into(),
                args: serde_json::json!({"path":"fixture"}),
            };
            await_approval(&bg_state, &bg_run, &call, "Earlier Ask decision", 1).await
        });
        let waiting = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.state, AgentState::WaitingPermission);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            save_auto_for_race_test(&state),
        )
        .await
        .expect("waiting for approval must not retain a permission read lock");
        assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 1);
        drop(update);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result,
            Some(ApprovalDecision::Approved { session: false })
        ));
        assert!(run.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_recheck_keeps_ask_waiting_for_an_explicit_decision() {
        let state = crate::api::AppState::new_stub();
        let run = approval_race_fixture(&state).await;
        let mut events = run.broadcaster.subscribe();
        let bg_state = state.clone();
        let bg_run = run.clone();
        let waiter = tokio::spawn(async move {
            let call = ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"echo fixture"}),
            };
            await_approval(&bg_state, &bg_run, &call, "Ask", 1).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        {
            let _update = state.settings_update.lock().await;
            assert_eq!(crate::api::resume_auto_approved_runs(&state).await, 0);
        }
        assert!(!waiter.is_finished());
        run.pending_tx
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(ApprovalDecision::Denied)
            .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Some(ApprovalDecision::Denied)));
        assert_eq!(
            state.permissions.read().await.autonomy,
            crate::permissions::AutonomyLevel::Assisted
        );
    }

    #[test]
    fn action_response_policy_allows_only_one_recovery_without_useful_progress() {
        let mut policy = ActionResponsePolicy::default();
        assert!(!policy.disable_native_thinking);
        assert_eq!(policy.output_budget(32768, 1000), 4096);
        assert!(
            policy.recover_invalid(),
            "the first invalid response gets a native retry with the format reminder"
        );
        assert!(policy.disable_native_thinking);
        assert!(!policy.structured_fallback);
        assert_eq!(policy.output_budget(32768, 1000), 8192);
        assert!(
            policy.recover_invalid(),
            "the second invalid response gets the schema-constrained envelope"
        );
        assert!(policy.structured_fallback);
        assert!(
            !policy.recover_invalid(),
            "changing from empty to malformed does not earn a third retry"
        );
        assert!(
            !policy.recover_invalid(),
            "repeated calls cannot restart recovery"
        );
        assert!(policy.disable_native_thinking);
    }

    #[test]
    fn action_response_policy_preserves_context_headroom_and_handles_extreme_counts() {
        let mut policy = ActionResponsePolicy::default();
        assert_eq!(policy.output_budget(4096, 2000), 1840);
        assert_eq!(policy.output_budget(4096, 3712), 128);
        assert_eq!(policy.output_budget(4096, 3800), 40);
        assert_eq!(policy.output_budget(4096, 4096), 0);
        assert_eq!(policy.output_budget(0, 0), 0);
        assert_eq!(policy.output_budget(u32::MAX, u32::MAX), 0);
        assert_eq!(policy.output_budget(4096, u32::MAX), 0);
        assert!(policy.recover_invalid());
        assert_eq!(
            policy.output_budget(4096, 2000),
            1840,
            "recovery cannot override context capacity"
        );
        assert_eq!(policy.output_budget(8192, 1000), 6936);
        assert_eq!(policy.output_budget(65536, 1000), 8192);
    }

    #[test]
    fn transcript_pruning_releases_old_results_before_dropping_turns() {
        let mut transcript = vec![
            ChatTurn::text("system", "instructions"),
            ChatTurn::text("user", "older history"),
            ChatTurn::text("assistant", "older answer"),
            ChatTurn::text("user", "Task: do the thing"),
            ChatTurn::text("assistant", "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"a\"}}\n```"),
            ChatTurn::text("user", format!("Result of read_file:\n{}", "a".repeat(6000))),
            ChatTurn::text("assistant", "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"b\"}}\n```"),
            ChatTurn::text("user", format!("Result of read_file:\n{}", "b".repeat(6000))),
            ChatTurn::text("assistant", "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"c\"}}\n```"),
            ChatTurn::text("user", format!("Result of read_file:\n{}", "c".repeat(200))),
        ];
        let mut task_index = 3;
        let before = transcript.clone();
        // Plenty of room: nothing changes.
        assert_eq!(prune_transcript(&mut transcript, &mut task_index, 32768, 4096), 0);
        assert_eq!(
            serde_json::to_value(&transcript).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        // ~3.2K tokens of results against a 4K window: release the oldest
        // result body first; the latest exchange and the task stay intact.
        let pruned = prune_transcript(&mut transcript, &mut task_index, 4096, 1024);
        assert!(pruned >= 1, "{pruned}");
        assert!(transcript[5].content.contains(RELEASED_MARKER));
        assert!(transcript[5].content.starts_with("Result of read_file:"));
        assert_eq!(transcript[task_index].content, "Task: do the thing");
        assert!(transcript.last().unwrap().content.contains(&"c".repeat(200)));
        assert_eq!(transcript[0].content, "instructions");
        // A window too small even for the released transcript drops the
        // saved history before the task, never the task itself.
        let mut tiny = transcript.clone();
        let mut tiny_task = task_index;
        prune_transcript(&mut tiny, &mut tiny_task, 1500, 256);
        assert_eq!(tiny[0].content, "instructions");
        assert_eq!(tiny[tiny_task].content, "Task: do the thing");
        assert!(tiny.len() >= tiny_task + 3, "latest exchange is protected");
    }

    #[test]
    fn complete_actions_stop_the_stream_but_partial_ones_do_not() {
        assert!(!action_complete("I will read the file first.\n```tool\n{\"name\":\"read_file\""));
        assert!(!action_complete("```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}"));
        // The object is complete: stop here rather than generate the closing
        // fence and whatever prose would follow it.
        assert!(action_complete("```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n"));
        assert!(action_complete(
            "I will read it.\n```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```"
        ));
        assert!(action_complete(
            "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```\n"
        ));
        assert!(!action_complete("<|tool_call>call:read_file{args:{\"path\":\"x\"}}"));
        assert!(action_complete("<|tool_call>call:read_file{args:{\"path\":\"x\"}}<tool_call|>"));
        assert!(!action_complete("Plain final answer without any action."));
    }

    #[test]
    fn action_response_policy_new_success_renews_retry_but_keeps_native_thinking_disabled() {
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        let args = serde_json::json!({"path":"README.md"});
        assert!(progress.begin_attempt());
        assert!(policy.recover_invalid());
        assert!(progress.begin_attempt());
        assert!(progress.observe_tool_result("read_file", &args, "actual fixture text", true).is_progress());
        policy.useful_progress();
        assert_eq!(progress.attempts_without_progress(), 0);
        assert!(
            policy.disable_native_thinking,
            "a successful action must not reactivate the strategy that exhausted its budget"
        );
        assert_eq!(policy.output_budget(32768, 1000), 8192);
        assert!(
            policy.recover_invalid(),
            "new host-confirmed evidence starts a new bounded recovery window"
        );
        assert!(policy.recover_invalid());
        assert!(!policy.recover_invalid());
        assert!(
            !ActionResponsePolicy::default().disable_native_thinking,
            "the override must not leak into another run"
        );
    }

    #[test]
    fn action_response_policy_failed_tool_does_not_renew_retry_but_execution_does() {
        // A failed tool leaves the failure budget consumed and the recovery
        // allowance spent.
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        let args = serde_json::json!({"path":"README.md"});
        assert!(progress.begin_attempt());
        assert!(policy.recover_invalid());
        assert!(progress.begin_attempt());
        assert!(!progress
            .observe_tool_result("read_file", &args, "not found", false)
            .is_progress());
        assert_eq!(progress.attempts_without_progress(), 2);
        assert!(policy.recover_invalid(), "the second strategy is still available");
        assert!(
            !policy.recover_invalid(),
            "a failed tool result is not grounds for another invalid-output retry"
        );
        // A successfully executed action, even a repeated read, is progress:
        // the failure budget resets and one more recovery is allowed. Looping
        // on identical results is bounded separately by the repeat guard.
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        assert!(progress
            .observe_tool_result("read_file", &args, "same text", true)
            .is_progress());
        assert!(progress.begin_attempt());
        assert!(policy.recover_invalid());
        assert!(progress.begin_attempt());
        let repeated = progress.observe_tool_result("read_file", &args, "same text", true);
        assert!(repeated.is_progress());
        policy.useful_progress();
        assert_eq!(progress.attempts_without_progress(), 0);
        assert!(policy.recover_invalid());
    }

    fn action_response_fixture(
        text: &str,
        finish_reason: &str,
    ) -> crate::llamaserver::AgentCompletion {
        crate::llamaserver::AgentCompletion {
            text: text.into(),
            metrics: crate::inference::Metrics::default(),
            finish_reason: Some(finish_reason.into()),
            reasoning_present: false,
            reasoning_tokens: None,
            native_tool_calls_present: false,
            early_stopped: false,
        }
    }

    #[test]
    fn continuation_joins_exact_text_and_validates_only_the_completed_action() {
        let prefix = "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"hello.txt\",\"content\":\"Hello ";
        let suffix = "world\\n\"}}\n```";
        let cutoff = action_response_fixture(prefix, "length");
        assert!(can_continue_response(&cutoff));
        assert!(parse_tool_block(prefix).is_none());
        let combined = assemble_continuation(prefix, suffix).unwrap();
        let completed = action_response_fixture(&combined, "stop");
        let parsed = parse_tool_block(&combined).unwrap();
        assert!(action_response_invalid_reason(&completed, Some(&parsed), 4096).is_none());
        assert_eq!(parsed.args["content"], "Hello world\n");
        let still_cutoff = action_response_fixture(&combined, "length");
        assert!(action_response_invalid_reason(&still_cutoff, Some(&parsed), 4096).is_some());
    }

    #[test]
    fn continuation_accepts_exact_prefix_replay_but_not_empty_or_duplicate_actions() {
        assert_eq!(
            assemble_continuation("Hello ", "Hello world").unwrap(),
            "Hello world"
        );
        assert!(assemble_continuation("Hello", "Hello").is_none());
        assert!(assemble_continuation("Hello", " \n").is_none());
        let action = "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```";
        let combined = assemble_continuation(action, &format!("\n{action}")).unwrap();
        assert!(
            parse_tool_block(&combined).is_none(),
            "continuation must not smuggle a second action"
        );
        assert!(assemble_continuation(&"a".repeat(32768), &"b".repeat(32769)).is_none());
    }

    #[test]
    fn continuation_carries_partial_text_in_context_without_mutating_base_history() {
        let base = vec![ChatTurn::text("user", "Create a small file.")];
        let prefix = "x".repeat(4000);
        let request = continuation_turns(&base, &prefix);
        assert_eq!(base.len(), 1);
        assert_eq!(request.len(), 3);
        assert_eq!(request[1].role, "assistant");
        assert_eq!(request[1].content, prefix);
        let before = crate::agent::AgentContextUsage::for_turns(&base, 4096, 0, 0);
        let after = crate::agent::AgentContextUsage::for_turns(&request, 4096, 0, 0);
        assert!(after.estimated_tokens > before.estimated_tokens + 1000);
    }

    #[test]
    fn continuation_is_only_for_bounded_visible_truncation_and_shares_retry_limit() {
        for completion in [
            action_response_fixture("", "length"),
            action_response_fixture("complete", "stop"),
            action_response_fixture(&"x".repeat(32769), "length"),
        ] {
            assert!(!can_continue_response(&completion));
        }
        let mut native = action_response_fixture("some content", "length");
        native.native_tool_calls_present = true;
        assert!(!can_continue_response(&native));
        let mut policy = ActionResponsePolicy::default();
        let mut progress = crate::agent_progress::ProgressGuard::new(4);
        assert!(progress.begin_attempt());
        assert!(policy.begin_continuation());
        assert!(policy.disable_native_thinking);
        assert!(progress.begin_attempt());
        assert!(
            !policy.begin_continuation(),
            "a continuation cannot chain another continuation"
        );
        assert!(
            policy.recover_invalid(),
            "a failed continuation permits a clean small-action fallback"
        );
        assert!(progress.begin_attempt());
        assert!(
            policy.recover_invalid(),
            "then the schema-constrained envelope"
        );
        assert!(
            !policy.recover_invalid(),
            "a failed constrained fallback must stop"
        );
        assert!(
            !policy.begin_continuation(),
            "format failures cannot reset the continuation budget"
        );
        assert_eq!(progress.attempts_without_progress(), 3);
        policy.useful_progress();
        assert!(
            policy.begin_continuation(),
            "only useful progress renews the allowance"
        );
    }

    #[test]
    fn action_response_validation_rejects_cutoff_even_when_the_tool_fence_is_parseable() {
        let text = "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"index.html\",\"content\":\"partial page\"}}\n```";
        let cutoff = action_response_fixture(text, "length");
        let parsed = parse_tool_block(&cutoff.text);
        assert!(
            parsed.is_some(),
            "syntactically complete payload deliberately exercises the provider cutoff guard"
        );
        let reason = action_response_invalid_reason(&cutoff, parsed.as_ref(), 2048).unwrap();
        assert!(reason.contains("2048-token output limit"));
        let mut complete = cutoff;
        complete.finish_reason = Some("stop".into());
        assert!(action_response_invalid_reason(&complete, parsed.as_ref(), 2048).is_none());
    }

    #[test]
    fn action_response_validation_distinguishes_empty_reasoning_native_calls_and_malformed_fences()
    {
        for text in ["", " ", "\n\t"] {
            let empty = action_response_fixture(text, "stop");
            assert!(action_response_invalid_reason(&empty, None, 2048)
                .unwrap()
                .contains("no visible"));
        }
        let mut hidden_only = action_response_fixture("", "stop");
        hidden_only.reasoning_present = true;
        assert!(action_response_invalid_reason(&hidden_only, None, 2048)
            .unwrap()
            .contains("reasoning but no visible"));
        hidden_only.native_tool_calls_present = true;
        assert!(action_response_invalid_reason(&hidden_only, None, 2048)
            .unwrap()
            .contains("unsupported action format"));
        for text in [
            "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"",
            "```tool\nnot json\n```",
            "```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n```tool\n{\"name\":\"write_file\",\"args\":{}}\n```",
        ] {
            let malformed = action_response_fixture(text, "stop");
            let parsed = parse_tool_block(text);
            assert!(parsed.is_none());
            assert!(action_response_invalid_reason(&malformed, parsed.as_ref(), 2048).unwrap().contains("incomplete or unreadable"));
        }
        let final_answer =
            action_response_fixture("The README describes a local notes app.", "stop");
        assert!(action_response_invalid_reason(&final_answer, None, 2048).is_none());
        let mut valid_tool = action_response_fixture(
            "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"}}\n```",
            "stop",
        );
        valid_tool.reasoning_present = true;
        let parsed = parse_tool_block(&valid_tool.text);
        assert!(
            action_response_invalid_reason(&valid_tool, parsed.as_ref(), 2048).is_none(),
            "reported reasoning is not itself a failure when the visible action completed"
        );
    }

    #[test]
    fn action_response_validation_alternating_empty_and_malformed_still_stops_after_two_retries() {
        let mut policy = ActionResponsePolicy::default();
        let responses = [
            action_response_fixture("", "length"),
            action_response_fixture("```tool\n{\"name\":\"read_file\"", "stop"),
            action_response_fixture("```tool\nnot json\n```", "stop"),
        ];
        let mut retries = 0;
        for response in &responses {
            let parsed = parse_tool_block(&response.text);
            assert!(action_response_invalid_reason(response, parsed.as_ref(), 2048).is_some());
            if policy.recover_invalid() {
                retries += 1;
            } else {
                break;
            }
        }
        assert_eq!(retries, 2);
        assert_eq!(policy.invalid_since_progress, 3);
    }

    #[test]
    fn unavailable_or_denied_search_is_failed_evidence_not_progress() {
        for output in [
            "(web search is OFF for this run — continue from local knowledge)",
            "(web search is disabled by network policy)",
            "(the user denied web search — continue from local knowledge)",
            "(web search failed: request timeout)",
            "(tool reported failure)\nprocess failed",
        ] {
            assert!(tool_output_failed(output), "{output}");
        }
        assert!(!tool_output_failed(
            "Web results via provider:\n1. Fixture result"
        ));
        assert!(!tool_output_failed("No search matches."));
    }

    #[test]
    fn native_gemma_action_normalizes_to_the_same_validated_tool() {
        let text = "<|tool_call>call:list_directory{args:{\"path\":\".\"}}<tool_call|>";
        let call = parse_action_response(text).unwrap();
        assert_eq!(call.name, "list_directory");
        assert_eq!(call.args, serde_json::json!({"path":"."}));
        assert_eq!(visible_progress(text), "");
        let cutoff = action_response_fixture(text, "length");
        assert!(action_response_invalid_reason(&cutoff, Some(&call), 2048).is_some());
    }

    #[test]
    fn structured_tools_and_final_answers_are_distinct_and_validated() {
        let action = r#"{"kind":"tool","name":"execute_command","args":{"command":"node verify.cjs"},"answer":""}"#;
        let call = parse_action_response(action).unwrap();
        assert_eq!(call.name, "execute_command");
        assert_eq!(call.args["command"], "node verify.cjs");
        assert!(visible_progress(action).is_empty());
        let final_text =
            r#"{"kind":"final","name":"","args":{},"answer":"Verified.\nAll checks passed."}"#;
        assert!(parse_action_response(final_text).is_none());
        assert_eq!(
            parse_structured_reply(final_text).unwrap().answer,
            "Verified.\nAll checks passed."
        );
        assert!(action_response_invalid_reason(
            &action_response_fixture(final_text, "stop"),
            None,
            4096
        )
        .is_none());
    }

    #[test]
    fn structured_envelope_rejects_ambiguous_duplicate_and_incomplete_fields() {
        for reply in [
            r#"{"kind":"tool","name":"read_file","args":[],"answer":""}"#,
            r#"{"kind":"tool","name":"","args":{},"answer":""}"#,
            r#"{"kind":"final","name":"","args":{},"answer":""}"#,
            r#"{"kind":"tool","name":"read_file","args":{},"answer":"","extra":1}"#,
            r#"{"kind":"tool","kind":"final","name":"read_file","args":{},"answer":""}"#,
            r#"{"kind":"tool","name":"read_file","args":{},"answer":""} {}"#,
        ] {
            assert!(parse_structured_reply(reply).is_none(), "accepted: {reply}");
            assert!(action_response_invalid_reason(
                &action_response_fixture(reply, "stop"),
                None,
                4096
            )
            .is_some());
        }
    }

    #[test]
    fn structured_kind_controls_execution_without_rejecting_unused_model_notes() {
        let tool = r#"{"kind":"tool","name":"read_file","args":{"path":"x"},"answer":"I will inspect this file."}"#;
        assert_eq!(parse_action_response(tool).unwrap().name, "read_file");
        let final_text = r#"{"kind":"final","name":"delete_file","args":{"path":"x"},"answer":"I could not complete it."}"#;
        assert!(
            parse_action_response(final_text).is_none(),
            "unused action metadata must never execute in a final response"
        );
        assert_eq!(
            parse_structured_reply(final_text).unwrap().answer,
            "I could not complete it."
        );
    }

    #[test]
    fn only_clean_recovery_enables_schema_and_success_keeps_the_working_protocol() {
        let mut policy = ActionResponsePolicy::default();
        assert!(!policy.structured_fallback);
        assert!(policy.begin_continuation());
        assert!(
            !policy.structured_fallback,
            "suffix continuation cannot be constrained as a whole JSON document"
        );
        assert!(policy.recover_invalid());
        assert!(
            !policy.structured_fallback,
            "the first recovery is a native retry with the format reminder"
        );
        assert!(policy.disable_native_thinking);
        assert!(policy.recover_invalid());
        assert!(policy.structured_fallback, "the second recovery constrains the envelope");
        assert!(!policy.recover_invalid(), "a third invalid reply ends the run");
        policy.useful_progress();
        assert!(policy.structured_fallback);
        let base = vec![ChatTurn::text("user", "Test")];
        let mut constrained = base.clone();
        constrained.push(ChatTurn::text("user", STRUCTURED_ACTION_INSTRUCTION));
        assert!(
            crate::agent::AgentContextUsage::for_turns(&constrained, 32768, 4096, 0)
                .estimated_tokens
                > crate::agent::AgentContextUsage::for_turns(&base, 32768, 4096, 0)
                    .estimated_tokens
        );
    }

    #[test]
    fn native_action_rejects_multiple_quoted_trailing_or_non_object_envelopes() {
        let valid = "<|tool_call>call:read_file{args:{\"path\":\"x\"}}<tool_call|>";
        for text in [
            format!("{valid}{valid}"),
            format!("Example: {valid}"),
            format!("{valid} explanation"),
            format!("```text\n{valid}\n```"),
            "<|tool_call>call:read_file{args:[]}<tool_call|>".into(),
            "<|tool_call>call:read_file{args:{\"path\":\"x\"}}".into(),
            "<|tool_call>call:read_file;command{args:{}}<tool_call|>".into(),
            "<|tool_call>call:read_file{args:{},another:{}}<tool_call|>".into(),
        ] {
            assert!(
                parse_action_response(&text).is_none(),
                "accepted ambiguous native action: {text}"
            );
            let response = action_response_fixture(&text, "stop");
            assert!(action_response_invalid_reason(&response, None, 4096).is_some());
        }
    }

    #[test]
    fn prompt_contains_a_complete_parseable_action_example() {
        let prompt = system_prompt("C:/fixture", &crate::tools::registry(), false, false);
        let start = prompt.find("```tool\n").unwrap();
        let end = prompt[start + 8..].find("\n```").unwrap() + start + 8 + 4;
        let example = &prompt[start..end];
        let call = parse_tool_block(example).unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "README.md");
        assert_eq!(
            prompt.matches("```").count(),
            4,
            "the prompt has two balanced examples"
        );
        // The second example teaches the raw-body form, so it has to parse
        // as one: a prompt that demonstrates an unreadable action is worse
        // than one that demonstrates nothing.
        let raw_start = prompt[end..].find("```tool\n").unwrap() + end;
        let raw_end = prompt[raw_start + 8..].find("\n```").unwrap() + raw_start + 8 + 4;
        let raw = parse_tool_block(&prompt[raw_start..raw_end]).unwrap();
        assert_eq!(raw.name, "write_file");
        assert_eq!(raw.args["path"], "src/app.js");
        assert_eq!(raw.args["content"], "const pattern = /\\d+/;\n");
    }

    #[test]
    fn source_text_mentioning_failure_markers_is_not_itself_failed_evidence() {
        for output in [
            "fn example() { let message = \"(tool error: fixture)\"; }",
            "Documentation: (tool reported failure) is an error marker.",
            "A test checks for (document failed: timeout).",
            "Search result: (web search failed: timeout) appears in this file.",
        ] {
            assert!(
                !tool_output_failed(output),
                "ordinary successful read/search text was classified as a host error: {output}"
            );
        }
    }

    #[test]
    fn read_only_modes_never_allow_mutating_risks() {
        for mode in [AgentMode::Plan, AgentMode::CodeAssist] {
            assert!(mode.allows_risk(RiskLevel::Safe));
            assert!(!mode.allows_risk(RiskLevel::Moderate));
            assert!(!mode.allows_risk(RiskLevel::Dangerous));
        }
        assert_eq!(
            serde_json::from_str::<AgentMode>("\"plan\"").unwrap(),
            AgentMode::Plan
        );
    }

    #[test]
    fn mutation_diff_records_actual_overwrite_and_not_proposed_changes() {
        let root = std::env::temp_dir().join(format!("companion-diff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let ws = crate::workspace::WorkspaceManager::new(root.clone());
        let call = ToolCall {
            name: "write_file".into(),
            args: serde_json::json!({"path": "sample.txt", "content": "proposed"}),
        };
        std::fs::write(root.join("sample.txt"), "before\n").unwrap();
        let before = mutation_snapshot(&ws, &call);
        assert!(mutation_diff(&ws, &call, before.clone()).is_none());
        std::fs::write(root.join("sample.txt"), "actual\n").unwrap();
        let diff = mutation_diff(&ws, &call, before).unwrap();
        assert!(diff.contains("--- a/sample.txt"));
        assert!(diff.contains("-before"));
        assert!(diff.contains("+actual"));
        assert!(!diff.contains("proposed"));
        std::fs::remove_file(root.join("sample.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn parses_one_standalone_tool_block_with_progress() {
        let text = "I'll inspect the entry point.\n```tool\n{\"name\": \"read_file\", \"args\": {\"path\": \"x\"}}\n```";
        let call = parse_tool_block(text).unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "x");
    }

    #[test]
    fn rejects_multiple_or_unfinished_action_fences() {
        let read = "```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"x\"}}\n```";
        let edit =
            "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"x\",\"content\":\"new\"}}\n```";
        for text in [
            format!("{read}\n{edit}"),
            format!("{read}\nThen edit:\n{edit}"),
            format!("{read}\n```tool\n{{\"name\":\"write_file\""),
            "```tool\n{\"name\":\"read_file\",\"args\":{".into(),
            "```tool\n{\"name\":\"read_file\",\"args\":{}}\nThen:\n```tool\n{\"name\":\"delete_file\",\"args\":{\"path\":\"x\"}}".into(),
        ] {
            assert!(
                parse_tool_block(&text).is_none(),
                "accepted ambiguous action: {text}"
            );
        }
    }

    #[test]
    fn rejects_quoted_nested_inline_and_trailing_example_markup() {
        for text in [
            "> ```tool\n> {\"name\":\"read_file\",\"args\":{}}\n> ```",
            "````markdown\n```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n````",
            "    ```tool\n    {\"name\":\"read_file\",\"args\":{}}\n    ```",
            " \t```tool\n{\"name\":\"read_file\",\"args\":{}}\n```",
            "Example: ```tool\n{\"name\":\"read_file\",\"args\":{}}\n```",
            "```tool\n{\"name\":\"read_file\",\"args\":{}} ```",
        ] {
            assert!(parse_tool_block(text).is_none(), "accepted quoted/example markup: {text}");
        }
    }

    #[test]
    fn javascript_escapes_inside_json_are_repaired_to_what_the_file_needs() {
        // The reply the rerun rejected: quotes and backticks escaped for
        // JavaScript inside a JSON string.
        let reply = "Now the script.\n```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"energy-drink/script.js\",\"content\":\"const hero = document.querySelector(\\'.hero-section\\');\\nstyle.textContent = \\`\\n.animate h1 { animation: fadeIn 2s; }\\n\\`;\\nconst re = /\\d+\\.\\d+/;\"}}\n```";
        let call = parse_tool_block(reply).expect("repaired");
        assert_eq!(
            call.args["content"],
            "const hero = document.querySelector('.hero-section');\nstyle.textContent = `\n.animate h1 { animation: fadeIn 2s; }\n`;\nconst re = /\\d+\\.\\d+/;"
        );
        assert!(action_problem(reply).is_none());
        // Valid escapes are untouched.
        let valid = "{\"a\":\"quote \\\" slash \\\\ tab \\t unicode \\u00e9\"}";
        assert_eq!(repair_json_strings(valid), valid);
    }

    #[test]
    fn repaired_multi_line_writes_keep_their_positions_in_the_original_reply() {
        // Raw newlines inside the content make the repaired copy longer than
        // the reply; slicing the reply with repaired offsets crashed a run.
        let html = "<!DOCTYPE html>\n<html>\n<body>\n<h1>Feel the Surge!</h1>\n</body>\n</html>";
        let object = format!("{{\"name\":\"write_file\",\"args\":{{\"path\":\"energy-drink/index.html\",\"content\":\"{html}\"}}}}");
        let open = format!("Creating the page.\n```tool\n{object}");
        let call = parse_tool_block(&open).expect("parsed after repair");
        assert_eq!(call.args["content"], html);
        assert_eq!(transcript_reply(&open), format!("{open}\n```"));
        assert_eq!(visible_progress(&open), "Creating the page.");
        let chatty = format!("{open}\nThe page is ready.");
        assert_eq!(transcript_reply(&chatty), format!("{open}\n```\nThe page is ready."));
        assert_eq!(visible_progress(&chatty), "Creating the page.\n\nThe page is ready.");
        let closed = format!("{open}\n```\nNext I will style it.");
        assert_eq!(parse_tool_block(&closed).unwrap().name, "write_file");
        assert_eq!(visible_progress(&closed), "Creating the page.\n\nNext I will style it.");
        let tagged = format!("<tool_call>\n{object}\n</tool_call>\nDone.");
        assert_eq!(visible_progress(&tagged), "Done.");
        let nested = "  {\"a\":\"}\n{\",\"b\":[1,{\"c\":2}]} tail";
        assert_eq!(&nested[..json_value_end(nested).unwrap()], "  {\"a\":\"}\n{\",\"b\":[1,{\"c\":2}]}");
        assert_eq!(json_value_end("prose {\"a\":1}"), None);
    }

    #[test]
    fn unreadable_actions_are_explained_in_the_parsers_words() {
        // A missing comma between fields: not repairable, so the parser's
        // own words come back.
        let text = "```tool\n{\"name\":\"create_document\" \"args\":{\"filename\":\"b.xlsx\"}}\n```";
        let problem = action_problem(text).expect("an attempt that does not parse");
        assert!(problem.contains("expected"), "{problem}");
        // The slips that are repaired: an unquoted spreadsheet formula, a
        // trailing comma, and both at once.
        let formula = "```tool\n{\"name\":\"create_document\",\"args\":{\"filename\":\"b.xlsx\",\"sheets\":[{\"name\":\"S\",\"rows\":[[\"Total\",=SUM(C2:C6)],[\"Avg\", =AVERAGE(C2, C6) ],]}],}}\n```";
        let call = parse_tool_block(formula).expect("repaired");
        assert_eq!(call.args["sheets"][0]["rows"][0][1], "=SUM(C2:C6)");
        assert_eq!(call.args["sheets"][0]["rows"][1][1], "=AVERAGE(C2, C6)");
        assert!(action_problem(formula).is_none());
        // Quoted formulas and equals signs inside strings are left alone.
        let quoted = "{\"a\":\"=SUM(1)\",\"b\":\"x=y\"}";
        assert_eq!(repair_json_strings(quoted), quoted);
        assert!(action_problem("Plain answer, no action.").is_none());
        assert!(action_problem("```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"a\"}}\n```").is_none());
        assert_eq!(
            action_problem("```tool\n{\"args\":{\"path\":\"a\"}}\n```").as_deref(),
            Some("the object has no \"name\" field")
        );
    }

    #[test]
    fn a_requested_document_is_only_verified_by_a_produced_file() {
        let mut state = VerificationState::for_task("make a short presentation about this project");
        assert!(state.needs_evidence());
        assert!(state.remaining().contains("pptx"));
        let ws = crate::workspace::WorkspaceManager::new(std::env::temp_dir());
        let read = ToolCall { name: "read_file".into(), args: serde_json::json!({"path":"README.md"}) };
        state.observe(&ws, &read, "contents", false);
        assert!(state.needs_evidence(), "reading is not producing");
        let failed = ToolCall { name: "create_document".into(), args: serde_json::json!({"filename":"x.pptx"}) };
        state.observe(&ws, &failed, "(tool error) no content", true);
        assert!(state.needs_evidence(), "a failed render is not a file");
        state.observe(&ws, &failed, "artifact 1: x.pptx (3 KB)", false);
        assert!(!state.needs_evidence());
        assert!(!VerificationState::for_task("explain the parser").needs_evidence());
    }

    #[test]
    fn raw_newlines_inside_json_strings_are_repaired_not_rejected() {
        // What a 14B coder actually emitted: a multi-line `old` with real
        // newlines inside the JSON string.
        let text = "```tool\n{\"name\":\"edit_file\",\"args\":{\"path\":\"calculator.js\",\"old\":\"function subtract(a, b) {\n  return a + b;\n}\",\"new\":\"function subtract(a, b) {\n  return a - b;\n}\"}}\n```";
        let call = parse_tool_block(text).expect("repaired JSON parses");
        assert_eq!(call.name, "edit_file");
        assert_eq!(call.args["old"], "function subtract(a, b) {\n  return a + b;\n}");
        assert_eq!(call.args["new"], "function subtract(a, b) {\n  return a - b;\n}");
        // Properly escaped JSON is untouched, and JSON that is broken in other
        // ways is still rejected.
        assert_eq!(repair_json_strings("{\"a\":\"x\\ny\"}"), "{\"a\":\"x\\ny\"}");
        assert!(parse_tool_block("```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\n```").is_none());
        let native = "<|tool_call>call:write_file{args:{\"path\":\"a.txt\",\"content\":\"line one\nline two\"}}<tool_call|>";
        assert_eq!(parse_action_response(native).unwrap().args["content"], "line one\nline two");
    }

    #[test]
    fn tool_parser_preserves_markdown_inside_json_file_contents() {
        let content = "Example:\n```tool\n{\"name\":\"read_file\",\"args\":{}}\n```\n";
        let payload =
            serde_json::json!({"name":"write_file","args":{"path":"README.md","content":content}});
        let text = format!("Writing the documentation…\r\n```tool\r\n{payload}\r\n```\r\n");
        let call = parse_tool_block(&text).unwrap();
        assert_eq!(call.name, "write_file");
        assert_eq!(call.args["content"], content);
    }

    #[test]
    fn tool_parser_requires_named_action_and_object_arguments() {
        for payload in [
            r#"{"name":"","args":{}}"#,
            r#"{"name":"read_file","args":[]}"#,
            r#"{"name":"read_file","args":"x"}"#,
            r#"{"name":"read_file","args":{}} {"name":"write_file","args":{}}"#,
        ] {
            assert!(parse_tool_block(&format!("```tool\n{payload}\n```")).is_none(), "{payload}");
        }
        // Absent or null arguments are an empty object: the tool's own
        // validation then tells the model exactly which argument it needs.
        for payload in [r#"{"name":"read_file"}"#, r#"{"name":"read_file","args":null}"#] {
            let call = parse_tool_block(&format!("```tool\n{payload}\n```")).unwrap();
            assert_eq!(call.args, serde_json::json!({}));
        }
    }

    #[test]
    fn no_block_means_final_answer() {
        assert!(parse_tool_block("All done. Summary here.").is_none());
        // A JSON snippet naming no registered tool is an illustration.
        assert!(parse_tool_block("```json\n{\"name\": \"x\"}\n```").is_none());
        assert!(parse_tool_block("```json\n{\"name\": \"my-app\",\"args\":{}}\n```").is_none());
        assert!(parse_tool_block("{\"name\": \"my-app\",\"args\":{}}").is_none());
        assert!(!looks_like_action_attempt("Done.\n```js\nconst name = 1;\n```"));
    }

    #[test]
    fn action_shapes_small_models_actually_produce_are_all_accepted() {
        // Fence never closed: the model ended its turn after the object.
        let open = "Fixing it.\n```tool\n{\"name\":\"edit_file\",\"args\":{\"path\":\"a.js\",\"old\":\"x\",\"new\":\"y\"}}";
        assert_eq!(parse_tool_block(open).unwrap().name, "edit_file");
        assert!(action_complete(open));
        assert_eq!(visible_progress(open), "Fixing it.");
        assert_eq!(transcript_reply(open), format!("{open}\n```"));
        // A closed fence keeps the transcript copy untouched.
        let closed = format!("{open}\n```\n");
        assert_eq!(transcript_reply(&closed), closed);
        // No fence at all and a narrative after the object (what an 8B model
        // writes before the tool has even run): the action is taken, the
        // narrative is the note, and the transcript copy gets the fence in
        // between.
        let chatty = format!("{open}\nThe spreadsheet has been created and is ready.");
        assert_eq!(parse_tool_block(&chatty).unwrap().name, "edit_file");
        assert!(action_complete(&chatty));
        assert_eq!(visible_progress(&chatty), "Fixing it.\n\nThe spreadsheet has been created and is ready.");
        assert_eq!(transcript_reply(&chatty), format!("{open}\n```\nThe spreadsheet has been created and is ready."));
        assert!(action_problem(&chatty).is_none());
        // Prose after the closing fence is a plan note, not a second action.
        let trailing = format!("{open}\n```\nThen I will run the tests.");
        assert_eq!(parse_tool_block(&trailing).unwrap().name, "edit_file");
        assert_eq!(visible_progress(&trailing), "Fixing it.\n\nThen I will run the tests.");
        // Two actions stay ambiguous.
        let twice = format!("{open}\n```\n{}", &open["Fixing it.\n".len()..]);
        assert!(parse_tool_block(&twice).is_none());
        assert!(looks_like_action_attempt(&twice));
        // An illustrative code block before the action is fine.
        let with_code = "Here is the fix:\n```js\nreturn a - b;\n```\n```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"a.js\"}}\n```";
        assert_eq!(parse_tool_block(with_code).unwrap().name, "read_file");
        assert_eq!(visible_progress(with_code), "Here is the fix:\n```js\nreturn a - b;\n```");
        // json and bare fences count when they name a registered tool;
        // `arguments` is accepted for `args`.
        let json_fence = "```json\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.js\"}}\n```";
        assert_eq!(parse_tool_block(json_fence).unwrap().args["path"], "a.js");
        let bare_fence = "```\n{\"tool\":\"list_directory\",\"args\":{\"path\":\".\"}}\n```";
        assert_eq!(parse_tool_block(bare_fence).unwrap().name, "list_directory");
        // A bare object without any fence.
        let bare = "{\"name\":\"read_file\",\"args\":{\"path\":\"a.js\"}}";
        assert_eq!(parse_action_response(bare).unwrap().name, "read_file");
        assert!(action_complete(bare));
        // The <tool_call> tag Qwen and Hermes models were trained on,
        // terminated or not.
        let tagged = "I'll read it.\n<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.js\"}}\n</tool_call>";
        assert_eq!(parse_action_response(tagged).unwrap().args["path"], "a.js");
        assert_eq!(visible_progress(tagged), "I'll read it.");
        let tagged_open = "<tool_call>{\"name\":\"read_file\",\"args\":{\"path\":\"a.js\"}}";
        assert!(action_complete(tagged_open));
        assert_eq!(transcript_reply(tagged_open), format!("{tagged_open}</tool_call>"));
        assert!(looks_like_action_attempt("<tool_call>\n{\"name\": \"read_file\""));
        // A labelled tool fence that never parses is an attempt, not an answer.
        assert!(parse_tool_block("```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\n```").is_none());
        assert!(looks_like_action_attempt("```json\n{\"name\":\"read_file\",\"args\":{\"path\":"));
    }

    #[test]
    fn visible_progress_strips_tool_payload() {
        let text = "I'll inspect the entry point.\n```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"src/main.rs\"}}\n```";
        assert_eq!(visible_progress(text), "I'll inspect the entry point.");
    }

    #[test]
    fn malformed_block_ignored() {
        assert!(parse_tool_block("```tool\nnot json\n```").is_none());
    }

    #[test]
    fn instructions_leave_a_small_window_most_of_its_history_room() {
        // On a 4K window the instructions are the part compaction can never
        // shrink, so they must stay under half of the room history gets.
        //
        // The bar was 45% until the raw-body form (rule 2b) and the shell
        // line were added. Both buy more room than they cost: without 2b a
        // small window cannot write a file at all, because the body has to
        // survive JSON escaping, and the shell line stops a run spending
        // steps on commands the host does not have. Everything redundant with
        // them was removed from the tool descriptions and rule 3 first.
        let prompt = system_prompt("C:/Users/someone/Code", &crate::tools::registry(), false, false);
        let tokens = estimated_tokens(&[ChatTurn::text("system", prompt)]);
        let room = history_room(4096, pruning_reserve(4096, 8192));
        assert!(tokens * 100 / room < 50, "{tokens} instruction tokens of {room}");
    }

    #[test]
    fn system_prompt_names_workspace_and_tools() {
        let p = system_prompt("C:/ws", &crate::tools::registry(), true, true);
        assert!(p.contains("C:/ws"));
        assert!(p.contains("read_file"));
        assert!(p.contains("```tool"));
        assert!(p.contains("web_search"));
        assert!(p.contains("create_document"));
        assert!(p.contains("unrelated sibling projects"));
        let q = system_prompt("C:/ws", &crate::tools::registry(), false, false);
        assert!(!q.contains("web_search"));
        // A coding task is not offered the document tool: a model used it to
        // "create a project" and to park page copy outside the site.
        assert!(!q.contains("create_document"));
        // Nor open_path, unless the user asked for something to be opened.
        assert!(!q.contains("open_path"));
        assert!(
            system_prompt_for("C:/ws", &crate::tools::registry(), false, false, true, 12_000)
                .contains("open_path")
        );
        for command in ["start energy-drink/index.html", "explorer .", "cmd /c start site.html", "Start-Process chrome", "xdg-open index.html", "powershell Start-Process index.html"] {
            assert!(launches_window(command), "{command}");
        }
        for command in ["npm start", "node server.js", "python -m http.server", "cargo run", "git status", "npm run start"] {
            assert!(!launches_window(command), "{command}");
        }
        assert!(opening_requested("build the page and open it in my browser"));
        assert!(opening_requested("Launch the dev server"));
        assert!(!opening_requested("create a modern looking website for an imaginary energy drink"));
    }

    #[test]
    fn follow_up_focus_excludes_unrelated_sibling_projects() {
        let root =
            std::env::temp_dir().join(format!("companion-project-focus-{}", uuid::Uuid::new_v4()));
        let ragev = root.join("RageV");
        let ember = root.join("ember");
        std::fs::create_dir_all(&ragev).unwrap();
        std::fs::create_dir_all(&ember).unwrap();
        std::fs::write(ragev.join("CMakeLists.txt"), "project(RageV)").unwrap();
        std::fs::write(ember.join("Cargo.toml"), "[package]").unwrap();

        let follow_up = vec![
            "What is the RageV project?".to_string(),
            "Find the tasks that are still pending in this project.".to_string(),
        ];
        assert_eq!(
            focused_workspace_root(&root, &follow_up),
            std::fs::canonicalize(&ragev).unwrap()
        );
        assert_eq!(
            focused_workspace_root(&root, &["Compare RageV and ember".into()]),
            root
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn completion_review_is_strict_about_inconclusive_answers() {
        assert!(matches!(
            parse_completion_review("COMPLETE"),
            CompletionReview::Complete
        ));
        assert!(matches!(
            parse_completion_review("CONTINUE: create the missing index.html"),
            CompletionReview::Continue(reason) if reason.contains("index.html")
        ));
        assert!(matches!(
            parse_completion_review("looks fine"),
            CompletionReview::Unavailable(_)
        ));
        assert!(matches!(
            parse_completion_review(""),
            CompletionReview::Unavailable(_)
        ));
    }

    #[test]
    fn tracks_changes_and_verification_actions() {
        let root = std::env::temp_dir().join(format!("verification-{}", uuid::Uuid::new_v4()));
        let ws = crate::workspace::WorkspaceManager::new(root);
        let mut state = VerificationState::default();
        let write = ToolCall {
            name: "write_file".into(),
            args: serde_json::json!({"path":"index.html"}),
        };
        let read = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"index.html"}),
        };
        let test = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"npm test"}),
        };
        state.observe(&ws, &write, "written", false);
        assert!(state.needs_evidence());
        state.observe(
            &ws,
            &ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path":"README.md"}),
            },
            "unrelated",
            false,
        );
        assert!(
            state.needs_evidence(),
            "an unrelated read must not verify a change"
        );
        state.observe(&ws, &read, "contents", false);
        assert!(!state.needs_evidence());
        state.observe(&ws, &write, "rewritten", false);
        assert!(
            state.needs_evidence(),
            "a new revision invalidates prior inspection"
        );
        state.observe(&ws, &test, "Command:\nnpm test\n\nExit code:\n1\n", true);
        assert!(state.needs_evidence());
        state.observe(
            &ws,
            &ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"npm test","cwd":"other"}),
            },
            "Command:\nnpm test\n\nExit code:\n0\n",
            false,
        );
        assert!(
            state.needs_evidence(),
            "tests in another directory do not verify this file"
        );
        state.observe(&ws, &test, "Command:\nnpm test\n\nExit code:\n0\n", false);
        assert!(!state.needs_evidence());
        assert!(tool_output_failed("Command:\nx\n\nExit code:\n1\n"));
        assert!(!tool_output_failed("Command:\nx\n\nExit code:\n0\n"));
        assert!(
            tool_output_failed("Command:\nx\n\nExit code:\n1\n\n--- stdout ---\nExit code:\n0\n"),
            "stdout text cannot forge the host process exit"
        );
    }

    #[test]
    fn named_node_verification_scripts_clear_only_successful_in_scope_evidence() {
        for command in [
            "node verify.cjs",
            "node scripts/check.mjs",
            "node test.js",
            "node feature.spec.cjs",
        ] {
            assert!(verification_command(command));
        }
        for command in [
            "node server.js",
            "node verify.cjs && node rewrite.js",
            "node verify.cjs; del important.txt",
            "echo node verify.cjs",
        ] {
            assert!(!verification_command(command));
        }
        let ws = crate::workspace::WorkspaceManager::new(std::env::temp_dir());
        let mut evidence = VerificationState {
            unobserved_dirs: [ws.resolve(".").unwrap()].into_iter().collect(),
            ..Default::default()
        };
        let call = ToolCall {
            name: "execute_command".into(),
            args: serde_json::json!({"command":"node verify.cjs","cwd":"."}),
        };
        evidence.observe(
            &ws,
            &call,
            "Command:\nnode verify.cjs\n\nExit code:\n1\n",
            true,
        );
        assert!(evidence.needs_evidence());
        evidence.observe(
            &ws,
            &call,
            "Command:\nnode verify.cjs\n\nExit code:\n0\n",
            false,
        );
        assert!(!evidence.needs_evidence());
    }

    #[test]
    fn verification_does_not_match_substrings_or_arbitrary_shell_changes() {
        for command in [
            "echo false",
            "echo check",
            "dir",
            "Get-Content test.txt",
            "python mutate.py",
            "npm test; Set-Content x y",
            "node not-a-test.js",
            "test -f x",
        ] {
            assert!(!verification_command(command), "{command}");
        }
        for command in [
            "npm test",
            "npm run typecheck",
            "cargo check",
            "python -m pytest",
            "node calculator.test.js",
            "node --test",
        ] {
            assert!(verification_command(command), "{command}");
        }
        let ws = crate::workspace::WorkspaceManager::new(std::env::temp_dir());
        let mut state = VerificationState::default();
        state.observe(
            &ws,
            &ToolCall {
                name: "execute_command".into(),
                args: serde_json::json!({"command":"python mutate.py"}),
            },
            "Exit code:\n1\n",
            true,
        );
        assert!(
            !state.unobserved_dirs.is_empty(),
            "even a failed process may partially write"
        );
        state.observe(
            &ws,
            &ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path":"other.txt"}),
            },
            "contents",
            false,
        );
        assert!(state.needs_evidence(), "an unrelated read does not show what a command did");
    }

    /// The sequence of the run that got stuck: `mkdir` plus two page files
    /// for a static website, which has no test or build to run. Completion
    /// must be reachable by looking at what was made.
    #[test]
    fn a_project_without_tests_can_finish_after_looking_at_what_changed() {
        let root = std::env::temp_dir().join(format!("verification-site-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("energy-drink")).unwrap();
        let ws = crate::workspace::WorkspaceManager::new(root.clone());
        let mut state = VerificationState::default();
        let call = |name: &str, args: serde_json::Value| ToolCall { name: name.into(), args };
        state.observe(&ws, &call("execute_command", serde_json::json!({"command":"mkdir energy-drink","cwd":"."})), "Command:\nmkdir energy-drink\n\nExit code:\n0\n", false);
        state.observe(&ws, &call("write_file", serde_json::json!({"path":"energy-drink/index.html"})), &format!("wrote 1275 bytes (40 lines) to energy-drink/index.html {}", crate::tools::WRITE_VERIFIED), false);
        state.observe(&ws, &call("write_file", serde_json::json!({"path":"energy-drink/style.css"})), &format!("wrote 758 bytes (30 lines) to energy-drink/style.css {}", crate::tools::WRITE_VERIFIED), false);
        assert!(state.needs_evidence(), "the command's folder has not been looked at");
        let remaining = state.remaining();
        assert!(remaining.contains("list \".\" (the workspace root)"), "{remaining}");
        assert!(!remaining.contains("index.html"), "host-confirmed writes need no read-back: {remaining}");
        // Listing a different folder does not show what ran in the root.
        state.observe(&ws, &call("list_directory", serde_json::json!({"path":"energy-drink"})), "index.html\nstyle.css", false);
        assert!(state.needs_evidence());
        state.observe(&ws, &call("list_directory", serde_json::json!({"path":"."})), "energy-drink", false);
        assert!(!state.needs_evidence(), "{}", state.remaining());

        // An unconfirmed write (a result without the marker) still needs a read,
        // an edit always does, and a shell listing counts like list_directory.
        let mut state = VerificationState::default();
        state.observe(&ws, &call("write_file", serde_json::json!({"path":"energy-drink/app.js"})), "wrote 10 bytes to energy-drink/app.js", false);
        state.observe(&ws, &call("edit_file", serde_json::json!({"path":"energy-drink/style.css"})), "patched energy-drink/style.css: 758 -> 790 bytes", false);
        state.observe(&ws, &call("execute_command", serde_json::json!({"command":"npm init -y","cwd":"energy-drink"})), "Command:\nnpm init -y\n\nExit code:\n0\n", false);
        let remaining = state.remaining();
        assert!(remaining.contains("\"energy-drink\""), "{remaining}");
        assert!(remaining.contains("energy-drink/app.js") && remaining.contains("energy-drink/style.css"), "{remaining}");
        state.observe(&ws, &call("execute_command", serde_json::json!({"command":"dir","cwd":"energy-drink"})), "Command:\ndir\n\nExit code:\n0\n", false);
        state.observe(&ws, &call("read_file", serde_json::json!({"path":"energy-drink/app.js"})), "x", false);
        state.observe(&ws, &call("read_file", serde_json::json!({"path":"energy-drink/style.css"})), "x", false);
        assert!(!state.needs_evidence(), "{}", state.remaining());

        // A script run in the root is observed by `git status` there too.
        let mut state = VerificationState::default();
        state.observe(&ws, &call("execute_command", serde_json::json!({"command":"python build_site.py"})), "Command:\npython build_site.py\n\nExit code:\n0\n", false);
        assert!(state.needs_evidence());
        state.observe(&ws, &call("execute_command", serde_json::json!({"command":"git status"})), "Command:\ngit status\n\nExit code:\n0\n", false);
        assert!(!state.needs_evidence());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn small_windows_keep_room_for_history_instead_of_reserving_all_of_it() {
        // The run that got stuck: a 4K window and the 8K cap used with thinking off.
        assert_eq!(pruning_reserve(4096, 8192), 1024);
        assert_eq!(pruning_reserve(4096, 4096), 1024);
        assert_eq!(pruning_reserve(8192, 8192), 2048);
        assert_eq!(pruning_reserve(32768, 4096), 4096, "large windows keep the full cap");
        assert_eq!(pruning_reserve(2048, 8192), 1024);
        // A 1.1K-token instruction block, the task and six short steps fit a
        // 4K window: nothing is dropped. Reserving the whole cap dropped all
        // but the latest exchange.
        let mut transcript = vec![
            ChatTurn::text("system", "r".repeat(4400)),
            ChatTurn::text("user", "Task: build the site"),
        ];
        for step in 0..6 {
            transcript.push(ChatTurn::text("assistant", format!("```tool\n{{\"name\":\"list_directory\",\"args\":{{\"path\":\"step{step}\"}}}}\n```")));
            transcript.push(ChatTurn::text("user", "Result of list_directory:\nindex.html"));
        }
        let before = transcript.len();
        let mut task_index = 1;
        assert_eq!(prune_transcript(&mut transcript, &mut task_index, 4096, pruning_reserve(4096, 8192)), 0);
        assert_eq!(transcript.len(), before);
        let mut old = transcript.clone();
        let mut old_index = 1;
        prune_transcript(&mut old, &mut old_index, 4096, 8192);
        assert_eq!(old.len(), 4, "the old reserve kept only the rules, the task and one exchange");
    }

    #[test]
    fn compaction_is_due_at_the_threshold_between_steps_only_when_there_is_history() {
        let policy = CompactionPolicy { enabled: true, threshold_pct: 90 };
        let room = history_room(4096, pruning_reserve(4096, 8192));
        assert_eq!(room, 2560);
        let mut transcript = vec![
            ChatTurn::text("system", "r".repeat(4000)),
            ChatTurn::text("user", "Task: build the site"),
        ];
        assert_eq!(policy.due(&transcript, 1, room, None), None, "nothing to summarize yet");
        while estimated_tokens(&transcript) * 100 < room * 90 {
            transcript.push(ChatTurn::text("assistant", "a".repeat(400)));
            transcript.push(ChatTurn::text("user", "Result of read_file:\nb"));
        }
        let used = estimated_tokens(&transcript);
        let pct = policy.due(&transcript, 1, room, None).expect("due at 90%");
        assert!(pct >= 90);
        // Right after a compaction that left the transcript this large, the
        // next one waits for a sixth of the room of growth.
        assert_eq!(policy.due(&transcript, 1, room, Some(used - 100)), None, "not grown enough since the last compaction");
        assert!(policy.due(&transcript, 1, room, Some(used - room / 6)).is_some());
        assert_eq!(CompactionPolicy { enabled: false, threshold_pct: 90 }.due(&transcript, 1, room, None), None);
        assert_eq!(CompactionPolicy { enabled: true, threshold_pct: 98 }.due(&transcript[..transcript.len() - 2], 1, room, None), None);
    }

    async fn note_sidecar(reply: &'static str) -> (String, Arc<Mutex<Vec<serde_json::Value>>>, JoinHandle<()>) {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = bodies.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(body);
                    axum::Json(serde_json::json!({
                        "choices": [{"message": {"content": reply}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 900, "completion_tokens": 60}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{port}"), bodies, server)
    }

    fn site_transcript(content_chars: usize) -> Vec<ChatTurn> {
        let mut transcript = vec![
            ChatTurn::text("system", "rules ".repeat(700)),
            ChatTurn::text("user", "Task: create a website"),
        ];
        for file in ["index.html", "style.css", "script.js"] {
            push_write(&mut transcript, file, content_chars);
        }
        transcript
    }

    fn push_write(transcript: &mut Vec<ChatTurn>, file: &str, content_chars: usize) {
        transcript.push(ChatTurn::text("assistant", format!("```tool\n{{\"name\":\"write_file\",\"args\":{{\"path\":\"site/{file}\",\"content\":\"{}\"}}}}\n```", "x".repeat(content_chars))));
        transcript.push(ChatTurn::text("user", format!("Result of write_file:\nwrote {content_chars} bytes (1 lines) to site/{file} (verified on disk)")));
    }

    #[tokio::test]
    async fn compaction_replaces_earlier_steps_with_a_note_and_the_host_record() {
        let (url, bodies, server) = note_sidecar("- Created site/index.html, site/style.css and site/script.js.\n- Next: list the site folder, then finish.").await;
        let client = SidecarClient::new(url).unwrap();
        // An 8K window, where the latest exchange fits and is kept verbatim.
        let cfg = crate::inference::InferenceConfig { n_ctx: 8192, ..Default::default() };
        let mut transcript = site_transcript(3000);
        let original_task = transcript[1].content.clone();
        let mut task_index = 1;
        let work_log = vec![
            "ran `mkdir site` in .: exit code 0".to_string(),
            "wrote 900 bytes (1 lines) to site/index.html (verified on disk)".to_string(),
        ];
        let required = vec!["Still required before finishing: Before finishing, list \".\" (the workspace root) with list_directory to see what your commands left there.".to_string()];
        let room = history_room(8192, pruning_reserve(8192, 8192));
        let outcome = compact_run_transcript(&client, &cfg, &mut transcript, &mut task_index, &original_task, &work_log, &required, room).await;
        let bodies = bodies.lock().unwrap().clone();
        server.abort();

        // The note request rides on the existing transcript, ends with the
        // instruction, and asks for no thinking.
        assert_eq!(bodies.len(), 1);
        let messages = bodies[0]["messages"].as_array().unwrap();
        assert_eq!(messages.last().unwrap()["content"], COMPACTION_INSTRUCTION);
        assert_eq!(bodies[0]["chat_template_kwargs"]["enable_thinking"], false);

        assert!(outcome.model_note);
        assert_eq!(outcome.summarized_turns, 4, "two older exchanges summarized, the latest kept");
        assert!(outcome.tokens_after < outcome.tokens_before);
        assert_eq!(task_index, 1);
        assert_eq!(transcript.len(), 4, "system, task with the note, and the latest exchange");
        assert_eq!(transcript[2].role, "assistant");
        assert!(transcript[3].content.contains("site/script.js"));
        let task = &transcript[1].content;
        assert!(task.starts_with(&original_task));
        assert!(task.contains("Context compacted between steps: 4 earlier turn(s)"));
        assert!(task.contains("Created site/index.html"));
        assert!(task.contains("- ran `mkdir site` in .: exit code 0"));
        assert!(task.contains("Still required before finishing"));
        assert!(task.contains("do not redo completed actions"));

        // A second compaction rebuilds the block instead of stacking notes.
        transcript.push(ChatTurn::text("assistant", "```tool\n{\"name\":\"list_directory\",\"args\":{\"path\":\".\"}}\n```"));
        transcript.push(ChatTurn::text("user", "Result of list_directory:\nsite"));
        push_write(&mut transcript, "about.html", 3000);
        push_write(&mut transcript, "extra.css", 3000);
        let (url, _, server) = note_sidecar("Site created and listed.").await;
        let client = SidecarClient::new(url).unwrap();
        compact_run_transcript(&client, &cfg, &mut transcript, &mut task_index, &original_task, &work_log, &[], room).await;
        server.abort();
        assert_eq!(transcript[1].content.matches("Context compacted between steps").count(), 1);
    }

    #[tokio::test]
    async fn the_completion_check_judges_the_work_without_its_earlier_verdicts() {
        let (url, bodies, server) = note_sidecar("COMPLETE").await;
        let client = SidecarClient::new(url).unwrap();
        let cfg = crate::inference::InferenceConfig { n_ctx: 8192, ..Default::default() };
        let mut transcript = site_transcript(300);
        transcript.push(ChatTurn::text("user", format!("{REVIEW_FINDING_PREFIX} the page has no hype line\nContinue executing now.")));
        transcript.push(ChatTurn::text("assistant", "```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"site/index.html\",\"content\":\"<p class='hype'>Unleash it</p>\"}}\n```"));
        transcript.push(ChatTurn::text("user", format!("{REVIEW_UNAVAILABLE_PREFIX} (timeout). Inspect the workspace.")));
        let evidence: Vec<String> = (0..40).map(|i| format!("- FILE CHANGE (not verification): write_file [site/page{i}.html] => wrote {} bytes", "9".repeat(300))).collect();
        let review = review_completion(&client, &cfg, &transcript, "create a website", &evidence).await;
        server.abort();
        assert!(matches!(review, CompletionReview::Complete));
        let bodies = bodies.lock().unwrap().clone();
        let messages = bodies[0]["messages"].as_array().unwrap();
        let contents: Vec<&str> = messages.iter().filter_map(|m| m["content"].as_str()).collect();
        assert!(!contents.iter().any(|c| c.starts_with(REVIEW_FINDING_PREFIX)), "earlier verdict leaked into the check");
        assert!(!contents.iter().any(|c| c.starts_with(REVIEW_UNAVAILABLE_PREFIX)));
        let instruction = contents.last().unwrap();
        assert!(instruction.contains("judge only its latest version"));
        assert!(instruction.contains("earlier actions omitted to fit the context window"), "evidence must be sized to the window");
    }

    #[tokio::test]
    async fn compaction_without_a_usable_note_still_keeps_the_host_record() {
        // A small model that answers the note request with an action.
        let (url, _, server) = note_sidecar("```tool\n{\"name\":\"list_directory\",\"args\":{\"path\":\".\"}}\n```").await;
        let client = SidecarClient::new(url).unwrap();
        let cfg = crate::inference::InferenceConfig { n_ctx: 4096, ..Default::default() };
        let mut transcript = site_transcript(900);
        let original_task = transcript[1].content.clone();
        let mut task_index = 1;
        let work_log = vec!["wrote 900 bytes (1 lines) to site/index.html (verified on disk)".to_string()];
        let room = history_room(4096, pruning_reserve(4096, 8192));
        let outcome = compact_run_transcript(&client, &cfg, &mut transcript, &mut task_index, &original_task, &work_log, &[], room).await;
        server.abort();
        assert!(!outcome.model_note);
        // On the incident's 4K window the instructions are close to half the
        // room, so even the latest exchange is summarized to land well under
        // the threshold.
        assert_eq!(outcome.summarized_turns, 6);
        assert_eq!(transcript.len(), 2);
        assert!(!transcript[1].content.contains("list_directory"));
        assert!(transcript[1].content.contains("site/index.html (verified on disk)"));
    }

    #[test]
    fn the_action_record_keeps_changes_and_commands_but_not_reads() {
        let call = |name: &str, args: serde_json::Value| ToolCall { name: name.into(), args };
        assert_eq!(
            work_log_entry(&call("execute_command", serde_json::json!({"command":"mkdir energy-drink","cwd":"."})), "Command:\nmkdir energy-drink\n\nExit code:\n0\n", false).as_deref(),
            Some("ran `mkdir energy-drink` in .: exit code 0")
        );
        assert_eq!(
            work_log_entry(&call("write_file", serde_json::json!({"path":"site/index.html"})), "wrote 12 bytes (1 lines) to site/index.html (verified on disk)", false).as_deref(),
            Some("wrote 12 bytes (1 lines) to site/index.html (verified on disk)")
        );
        assert_eq!(work_log_entry(&call("edit_file", serde_json::json!({"path":"a.css"})), "patched", false).as_deref(), Some("edited a.css"));
        assert!(work_log_entry(&call("edit_file", serde_json::json!({"path":"a.css"})), "(tool error, do not retry identically)\nold text was not found", true).unwrap().contains("failed: old text was not found"));
        assert_eq!(work_log_entry(&call("read_file", serde_json::json!({"path":"a.css"})), "x", false), None);
        assert_eq!(work_log_entry(&call("list_directory", serde_json::json!({"path":"."})), "x", false), None);
    }

    #[test]
    fn identical_failures_are_bounded_but_corrected_arguments_can_progress() {
        let call = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"missing.txt"}),
        };
        let corrected = ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path":"actual.txt"}),
        };
        let mut failures = FailedCalls::default();
        assert!(!failures.observe(&call, true));
        assert!(!failures.observe(&corrected, true));
        assert!(!failures.observe(&call, true));
        assert!(failures.observe(&call, true));
        assert!(!failures.observe(&call, false));
        assert!(!failures.observe(&call, true));
    }
}
