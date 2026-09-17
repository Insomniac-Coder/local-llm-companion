//! Splits a model's streamed reply into what the user sees and the actions
//! (tool envelopes) the host runs.
//!
//! llama-server delivers everything as one content stream, so a tool call a
//! model writes arrives as text. Without this, envelopes, raw file bodies and
//! malformed attempts streamed into the chat bubble and only a frontend regex
//! hid some of them afterwards. The splitter decides as the text arrives,
//! with the same recognisers the action parser uses, so what is hidden and
//! what is executed can never disagree:
//!
//! - prose is released at once;
//! - text that could open an action is held until it is decided: a line
//!   beginning with backticks, `<tool_call>` / `<|tool_call>` (or a partial
//!   one at the end of the text), or a reply that begins with `{`;
//! - a ```` ```tool ```` fence is an action from its first line; a ```` ```json ````
//!   or bare fence, or a leading `{`, is an action only when its complete body
//!   parses as a call to a known tool, and is otherwise released unchanged as
//!   an ordinary code block (a few tokens late, never dropped);
//! - raw `<<<NAME … NAME>>>` bodies inside an action are skipped whole, so a
//!   fence inside file content cannot end the action early.
//!
//! Actions surface only as `Action*` pieces carrying the tool name, never
//! their bodies.

use crate::agent_runner::{fence_opener, known_tool, parse_action_with_raw};

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Prose(String),
    ActionStarted { name: Option<String> },
    ActionFinished { name: Option<String> },
    /// The stream ended inside an action that never closed.
    ActionIncomplete { name: Option<String> },
}

const TAG_OPEN: &str = "<tool_call>";
const TAG_CLOSE: &str = "</tool_call>";
const GEMMA_OPEN: &str = "<|tool_call>";
const GEMMA_CLOSE: &str = "<tool_call|>";
/// A leading `{` or a json/bare fence body this long without a `"name"` key is
/// released as ordinary text.
const UNDECIDED_LIMIT: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Prose,
    /// Inside an ordinary code block (a non-action fence): released as prose.
    CodeBlock,
    /// Inside a ```tool fence.
    FencedAction,
    /// A ```json or bare fence whose body is still undecided.
    MaybeFenced,
    /// `<tool_call>` or `<|tool_call>` until its closing tag.
    TaggedAction { gemma: bool },
    /// The reply began with `{`; undecided until the object is complete.
    MaybeBareJson,
}

#[derive(Debug)]
pub struct StreamSplitter {
    /// Received, not yet released or consumed.
    pending: String,
    /// `pending` begins at the start of a line.
    at_line_start: bool,
    /// Any non-whitespace text has been released or consumed.
    reply_started: bool,
    mode: Mode,
    /// For MaybeFenced: the opener line, kept to release it if not an action.
    opener: String,
    started: bool,
    name: Option<String>,
}

impl Default for StreamSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamSplitter {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            at_line_start: true,
            reply_started: false,
            mode: Mode::Prose,
            opener: String::new(),
            started: false,
            name: None,
        }
    }

    pub fn push(&mut self, delta: &str) -> Vec<Piece> {
        self.pending.push_str(delta);
        let mut out = Vec::new();
        self.drain(false, &mut out);
        merge_prose(out)
    }

    /// The stream ended: resolve whatever is still held.
    pub fn finish(&mut self) -> Vec<Piece> {
        let mut out = Vec::new();
        self.drain(true, &mut out);
        merge_prose(out)
    }

    fn release(&mut self, len: usize, out: &mut Vec<Piece>) {
        if len == 0 {
            return;
        }
        let text: String = self.pending.drain(..len).collect();
        self.note_consumed(&text);
        out.push(Piece::Prose(text));
    }

    fn consume(&mut self, len: usize) -> String {
        let text: String = self.pending.drain(..len).collect();
        self.note_consumed(&text);
        text
    }

    fn note_consumed(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.at_line_start = text.ends_with('\n');
        if text.chars().any(|c| !c.is_whitespace()) {
            self.reply_started = true;
        }
    }

    fn begin_action(&mut self, mode: Mode, out: &mut Vec<Piece>) {
        self.mode = mode;
        self.started = false;
        self.name = None;
        let _ = out;
    }

    fn announce(&mut self, action_text: &str, out: &mut Vec<Piece>, force: bool) {
        if self.started {
            return;
        }
        if self.name.is_none() {
            self.name = action_name(action_text);
        }
        if self.name.is_some() || force {
            self.started = true;
            out.push(Piece::ActionStarted { name: self.name.clone() });
        }
    }

    fn finish_action(&mut self, out: &mut Vec<Piece>, complete: bool) {
        if !self.started {
            self.started = true;
            out.push(Piece::ActionStarted { name: self.name.clone() });
        }
        out.push(if complete {
            Piece::ActionFinished { name: self.name.take() }
        } else {
            Piece::ActionIncomplete { name: self.name.take() }
        });
        self.started = false;
        self.mode = Mode::Prose;
    }

    fn drain(&mut self, finished: bool, out: &mut Vec<Piece>) {
        // Each pass either makes progress (releases, consumes or changes
        // mode) or returns to wait for more text.
        loop {
            let progressed = match self.mode {
                Mode::Prose => self.step_prose(finished, out),
                Mode::CodeBlock => self.step_code_block(finished, out),
                Mode::FencedAction => self.step_fenced_action(finished, out),
                Mode::MaybeFenced => self.step_maybe_fenced(finished, out),
                Mode::TaggedAction { gemma } => self.step_tagged(gemma, finished, out),
                Mode::MaybeBareJson => self.step_bare_json(finished, out),
            };
            if !progressed || self.pending.is_empty() && self.mode == Mode::Prose {
                if finished && !self.pending.is_empty() && self.mode == Mode::Prose {
                    let len = self.pending.len();
                    self.release(len, out);
                }
                return;
            }
        }
    }

    fn step_prose(&mut self, finished: bool, out: &mut Vec<Piece>) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        // A reply that begins with `{` may be a bare action object.
        if !self.reply_started {
            let lead = self.pending.len() - self.pending.trim_start().len();
            if self.pending[lead..].starts_with('{') {
                if lead > 0 {
                    let whitespace = self.consume(lead);
                    let _ = whitespace;
                }
                self.mode = Mode::MaybeBareJson;
                return true;
            }
            if self.pending.trim().is_empty() && !finished {
                return false;
            }
        }
        // The earliest point that could open an action.
        let tag = [TAG_OPEN, GEMMA_OPEN]
            .iter()
            .filter_map(|marker| self.pending.find(marker).map(|at| (at, *marker)))
            .min_by_key(|(at, _)| *at);
        let fence = self.first_fence_line();
        let partial_tag = if finished { None } else { partial_marker_suffix(&self.pending) };
        let candidates = [
            tag.map(|(at, _)| at),
            fence.map(|(at, _)| at),
            partial_tag,
        ];
        let Some(at) = candidates.iter().flatten().min().copied() else {
            let len = self.pending.len();
            self.release(len, out);
            return true;
        };
        if at > 0 {
            self.release(at, out);
            return true;
        }
        // The candidate is at the start of `pending`.
        if let Some((0, marker)) = tag {
            let gemma = marker == GEMMA_OPEN;
            self.begin_action(Mode::TaggedAction { gemma }, out);
            return true;
        }
        if let Some((0, complete_line)) = fence {
            let Some(line_len) = complete_line else {
                // An incomplete line that starts with backticks: wait for the
                // label, unless the stream ended.
                if finished {
                    let len = self.pending.len();
                    self.release(len, out);
                    return true;
                }
                return false;
            };
            let line = self.pending[..line_len].to_string();
            let label = line.trim().trim_start_matches('`').trim().to_ascii_lowercase();
            match fence_opener(&line) {
                Some(true) if label == "tool" => {
                    self.consume(line_len);
                    self.begin_action(Mode::FencedAction, out);
                }
                Some(true) => {
                    self.opener = self.consume(line_len);
                    self.mode = Mode::MaybeFenced;
                }
                _ => {
                    self.release(line_len, out);
                    self.mode = Mode::CodeBlock;
                }
            }
            return true;
        }
        // A partial tag marker at the very start: wait for more text.
        false
    }

    /// The first line of `pending` that starts (after up to three spaces)
    /// with a backtick: its byte offset, and its length including the newline
    /// when the line is complete.
    fn first_fence_line(&self) -> Option<(usize, Option<usize>)> {
        let mut offset = 0;
        let mut line_start = self.at_line_start;
        for line in self.pending.split_inclusive('\n') {
            if line_start {
                let indent = line.bytes().take_while(|b| *b == b' ').count();
                let rest = &line[indent..];
                let complete = line.ends_with('\n');
                // A fence is three backticks. An unfinished line of one or two
                // backticks may still become one; `x` at a line start never.
                let could_be_fence = rest.starts_with("```")
                    || (!complete && !rest.is_empty() && rest.chars().all(|c| c == '`'));
                if indent <= 3 && could_be_fence {
                    return Some((offset, complete.then_some(line.len())));
                }
            }
            offset += line.len();
            line_start = true;
        }
        None
    }

    fn step_code_block(&mut self, finished: bool, out: &mut Vec<Piece>) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let mut offset = 0;
        let mut line_start = self.at_line_start;
        for line in self.pending.split_inclusive('\n') {
            let complete = line.ends_with('\n');
            if line_start && complete && line.trim() == "```" {
                let end = offset + line.len();
                self.release(end, out);
                self.mode = Mode::Prose;
                return true;
            }
            if !complete {
                // Release up to this partial line; keep it only when it might
                // be the closing fence.
                let hold = line_start && line.trim_start().starts_with('`') && !finished;
                let end = if hold { offset } else { offset + line.len() };
                if end == 0 {
                    return false;
                }
                self.release(end, out);
                return !hold || end > 0;
            }
            offset += line.len();
            line_start = true;
        }
        let len = self.pending.len();
        self.release(len, out);
        true
    }

    /// Where a fence body closes: the byte offset just past the closing fence
    /// line, skipping raw `<<<NAME … NAME>>>` blocks. A closing fence without
    /// its newline closes only at the end of the stream: otherwise the newline
    /// that follows would be released as a stray blank line.
    fn fence_close(body: &str, finished: bool) -> Option<usize> {
        let mut offset = 0;
        let mut raw_terminator: Option<String> = None;
        for line in body.split_inclusive('\n') {
            let trimmed = line.trim();
            offset += line.len();
            if let Some(terminator) = &raw_terminator {
                if trimmed == terminator {
                    raw_terminator = None;
                }
                continue;
            }
            if let Some(name) = trimmed.strip_prefix("<<<") {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    raw_terminator = Some(format!("{name}>>>"));
                    continue;
                }
            }
            if trimmed == "```" || (trimmed.ends_with("```") && !trimmed.starts_with("```")) {
                if !line.ends_with('\n') && !finished {
                    return None;
                }
                return Some(offset);
            }
        }
        None
    }

    fn step_fenced_action(&mut self, finished: bool, out: &mut Vec<Piece>) -> bool {
        let action_text = self.pending.clone();
        self.announce(&action_text, out, false);
        match Self::fence_close(&self.pending, finished) {
            Some(end) => {
                self.consume(end);
                self.finish_action(out, true);
                true
            }
            None if finished => {
                // Stopped early after a complete call (the runtime is released
                // as soon as one arrives), or cut off inside it.
                let complete =
                    !raw_block_open(&self.pending) && parse_action_with_raw(self.pending.trim()).is_some();
                let len = self.pending.len();
                self.consume(len);
                self.finish_action(out, complete);
                true
            }
            None => false,
        }
    }

    fn step_maybe_fenced(&mut self, finished: bool, out: &mut Vec<Piece>) -> bool {
        let first = self.pending.trim_start().chars().next();
        let release_as_code = |this: &mut Self, out: &mut Vec<Piece>| {
            let opener = std::mem::take(&mut this.opener);
            out.push(Piece::Prose(opener));
            this.mode = Mode::CodeBlock;
        };
        // Not an object, or long without a name key: an ordinary code block.
        if first.is_some_and(|c| c != '{')
            || (self.pending.len() > UNDECIDED_LIMIT && !self.pending.contains("\"name\""))
        {
            release_as_code(self, out);
            return true;
        }
        let Some(end) = Self::fence_close(&self.pending, finished) else {
            if finished {
                if self.pending.contains("\"name\"") {
                    let len = self.pending.len();
                    let text = self.consume(len);
                    self.announce(&text, out, true);
                    self.opener.clear();
                    self.finish_action(out, parse_action_with_raw(text.trim()).is_some());
                } else {
                    release_as_code(self, out);
                }
                return true;
            }
            return false;
        };
        let block = &self.pending[..end];
        let body_end = block.trim_end().rfind("```").unwrap_or(block.len());
        let body = block[..body_end].trim();
        if is_action(body) {
            let text = self.consume(end);
            self.opener.clear();
            self.announce(&text, out, true);
            self.finish_action(out, true);
        } else {
            release_as_code(self, out);
        }
        true
    }

    fn step_tagged(&mut self, gemma: bool, finished: bool, out: &mut Vec<Piece>) -> bool {
        let close = if gemma { GEMMA_CLOSE } else { TAG_CLOSE };
        let action_text = self.pending.clone();
        self.announce(&action_text, out, false);
        match self.pending.find(close) {
            Some(at) => {
                self.consume(at + close.len());
                self.finish_action(out, true);
                true
            }
            None if finished => {
                let len = self.pending.len();
                self.consume(len);
                self.finish_action(out, false);
                true
            }
            None => false,
        }
    }

    fn step_bare_json(&mut self, finished: bool, out: &mut Vec<Piece>) -> bool {
        let mut values = serde_json::Deserializer::from_str(&self.pending).into_iter::<serde_json::Value>();
        match values.next() {
            Some(Ok(_)) => {
                let end = values.byte_offset();
                if is_action(&self.pending[..end]) {
                    let text = self.consume(end);
                    self.announce(&text, out, true);
                    self.finish_action(out, true);
                } else {
                    self.release(end, out);
                    self.mode = Mode::Prose;
                }
                true
            }
            Some(Err(error)) if error.is_eof() && !finished && self.pending.len() <= UNDECIDED_LIMIT => false,
            Some(Err(error)) if error.is_eof() && !finished && self.pending.contains("\"name\"") => false,
            Some(Err(_)) if finished && self.pending.contains("\"name\"") => {
                let len = self.pending.len();
                let text = self.consume(len);
                self.announce(&text, out, true);
                self.finish_action(out, false);
                true
            }
            _ => {
                // Not a JSON object after all, or too long to be an action.
                self.mode = Mode::Prose;
                self.reply_started = true;
                if finished {
                    let len = self.pending.len();
                    self.release(len, out);
                }
                true
            }
        }
    }
}

/// A raw `<<<NAME` block was opened and its `NAME>>>` terminator has not arrived.
fn raw_block_open(body: &str) -> bool {
    let mut open: Option<String> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        match &open {
            Some(terminator) if trimmed == terminator => open = None,
            Some(_) => {}
            None => {
                if let Some(name) = trimmed.strip_prefix("<<<") {
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && line.ends_with('\n')
                    {
                        open = Some(format!("{name}>>>"));
                    }
                }
            }
        }
    }
    open.is_some()
}

/// A complete action body: a call to a known tool (or a structured envelope)
/// and nothing after it.
fn is_action(body: &str) -> bool {
    let body = body.trim();
    parse_action_with_raw(body).is_some_and(|(call, consumed, enveloped)| {
        (enveloped || known_tool(&call.name)) && body[consumed..].trim().is_empty()
    })
}

/// The tool name as soon as the action text shows it.
fn action_name(text: &str) -> Option<String> {
    if let Some(rest) = text.trim_start().strip_prefix(GEMMA_OPEN) {
        let rest = rest.strip_prefix("call:")?;
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        return (!name.is_empty() && rest.len() > name.len()).then_some(name);
    }
    for key in ["\"name\"", "\"tool\""] {
        let Some(at) = text.find(key) else { continue };
        let rest = text[at + key.len()..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        let name = &rest[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some(name.to_string());
        }
    }
    None
}

/// The start of a suffix of `text` that is a proper prefix of a tag marker
/// (a `<tool_c` cut off by the transport), if any.
fn partial_marker_suffix(text: &str) -> Option<usize> {
    [TAG_OPEN, GEMMA_OPEN]
        .iter()
        .filter_map(|marker| {
            (1..marker.len())
                .rev()
                .find(|len| text.ends_with(&marker[..*len]))
                .map(|len| text.len() - len)
        })
        .min()
}

fn merge_prose(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match (out.last_mut(), piece) {
            (_, Piece::Prose(text)) if text.is_empty() => {}
            (Some(Piece::Prose(previous)), Piece::Prose(text)) => previous.push_str(&text),
            (_, piece) => out.push(piece),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `text` in `chunk`-character pieces; return the visible text and
    /// the action pieces.
    fn run(text: &str, chunk: usize) -> (String, Vec<Piece>) {
        let mut splitter = StreamSplitter::new();
        let mut pieces = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        for part in chars.chunks(chunk.max(1)) {
            pieces.extend(splitter.push(&part.iter().collect::<String>()));
        }
        pieces.extend(splitter.finish());
        let mut visible = String::new();
        let mut actions = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Prose(text) => visible.push_str(&text),
                other => actions.push(other),
            }
        }
        (visible, actions)
    }

    fn every_chunking(text: &str) -> (String, Vec<Piece>) {
        let reference = run(text, text.chars().count().max(1));
        for chunk in [1, 2, 3, 7, 13] {
            assert_eq!(run(text, chunk), reference, "chunk size {chunk} changed the result for {text:?}");
        }
        reference
    }

    fn started(name: &str) -> Piece {
        Piece::ActionStarted { name: Some(name.into()) }
    }

    fn finished(name: &str) -> Piece {
        Piece::ActionFinished { name: Some(name.into()) }
    }

    #[test]
    fn prose_passes_through_unchanged() {
        let text = "A linked list stores nodes.\n\nEach node points to the next — ünïcödé.";
        assert_eq!(every_chunking(text), (text.to_string(), vec![]));
    }

    #[test]
    fn a_fenced_action_is_hidden_and_named() {
        let text = "I will read it first.\n```tool\n{\"name\":\"read_file\",\"args\":{\"path\":\"src/main.rs\"}}\n```\n";
        let (visible, actions) = every_chunking(text);
        assert_eq!(visible, "I will read it first.\n");
        assert_eq!(actions, vec![started("read_file"), finished("read_file")]);
    }

    #[test]
    fn an_ordinary_code_block_is_shown_whole() {
        let text = "Here:\n```python\ndef add(a, b):\n    return a + b\n```\nIt returns the sum.";
        assert_eq!(every_chunking(text), (text.to_string(), vec![]));
    }

    #[test]
    fn a_json_example_that_is_not_a_tool_call_stays_visible() {
        let text = "The config looks like this:\n```json\n{\"port\": 8080, \"debug\": true}\n```\nDone.";
        assert_eq!(every_chunking(text), (text.to_string(), vec![]));
    }

    #[test]
    fn a_json_fence_holding_a_real_call_is_an_action() {
        let text = "```json\n{\"name\":\"list_directory\",\"args\":{\"path\":\".\"}}\n```";
        let (visible, actions) = every_chunking(text);
        assert_eq!(visible, "");
        assert_eq!(actions, vec![started("list_directory"), finished("list_directory")]);
    }

    #[test]
    fn a_raw_file_body_with_its_own_fences_does_not_end_the_action() {
        let text = "Writing the readme.\n```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"README.md\"}}\n<<<CONTENT\n# Title\n```bash\ncargo run\n```\nCONTENT>>>\n```\nDone.";
        let (visible, actions) = every_chunking(text);
        assert_eq!(visible, "Writing the readme.\nDone.");
        assert_eq!(actions, vec![started("write_file"), finished("write_file")]);
    }

    #[test]
    fn tagged_and_gemma_calls_are_hidden() {
        let (visible, actions) = every_chunking("Checking.<tool_call>{\"name\":\"read_file\",\"args\":{\"path\":\"a\"}}</tool_call> ok");
        assert_eq!(visible, "Checking. ok");
        assert_eq!(actions, vec![started("read_file"), finished("read_file")]);
        let (visible, actions) = every_chunking("<|tool_call>call:read_file{args:{path:\"a\"}}<tool_call|>");
        assert_eq!(visible, "");
        assert_eq!(actions, vec![started("read_file"), finished("read_file")]);
    }

    #[test]
    fn a_reply_that_is_only_an_action_object_is_hidden_but_other_json_is_not() {
        let (visible, actions) = every_chunking("{\"name\":\"read_file\",\"args\":{\"path\":\"a.txt\"}}");
        assert_eq!(visible, "");
        assert_eq!(actions, vec![started("read_file"), finished("read_file")]);
        let text = "{\"answer\": 42} is the object you asked for.";
        assert_eq!(every_chunking(text).0, text);
    }

    #[test]
    fn an_envelope_cut_off_by_the_end_of_the_stream_is_reported_incomplete() {
        let text = "Saving.\n```tool\n{\"name\":\"write_file\",\"args\":{\"path\":\"a.txt\"}}\n<<<CONTENT\nhalf a fi";
        let (visible, actions) = every_chunking(text);
        assert_eq!(visible, "Saving.\n");
        assert_eq!(actions, vec![started("write_file"), Piece::ActionIncomplete { name: Some("write_file".into()) }]);
    }

    #[test]
    fn inline_code_at_the_start_of_a_line_is_not_a_fence() {
        let text = "`x` is the input.\n`y` is the output.";
        assert_eq!(every_chunking(text), (text.to_string(), vec![]));
    }

    #[test]
    fn a_lone_backtick_or_angle_bracket_at_the_end_is_released_when_the_stream_ends() {
        assert_eq!(every_chunking("Use `x` or\n`").0, "Use `x` or\n`");
        assert_eq!(every_chunking("a < b and c <").0, "a < b and c <");
    }

    #[test]
    fn the_recorded_fixture_streams_split_as_expected() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/streams");
        let text_of = |name: &str| -> String {
            let expect: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.expect.json"))).unwrap()).unwrap();
            expect["text"].as_str().unwrap().to_string()
        };
        let (visible, actions) = every_chunking(&text_of("fenced_action"));
        assert_eq!(visible, "I will read the file first.\n");
        assert_eq!(actions, vec![started("read_file"), finished("read_file")]);
        let code = text_of("code_block_answer");
        assert_eq!(every_chunking(&code), (code.clone(), vec![]));
    }
}
