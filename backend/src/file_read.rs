//! Bounded, addressable text reads. Limits apply to a chunk, never to its location.
use crate::tools::{ToolError, ToolResult};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::Path;

const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 500;
const CHUNK_CHARS: usize = 12_000;

fn positive(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                ToolError::InvalidArgs(format!("{key} must be a positive integer (1-based)"))
            }),
    }
}

pub fn read(path: &Path, args: &Value) -> Result<ToolResult, ToolError> {
    let start = positive(args, "start_line", 1)?;
    let end = positive(args, "end_line", start.saturating_add(DEFAULT_LINES - 1))?;
    let column = positive(args, "start_column", 1)?;
    if end < start {
        return Err(ToolError::InvalidArgs(
            "end_line must be >= start_line".into(),
        ));
    }
    let end = end.min(start.saturating_add(MAX_LINES - 1));
    let file = std::fs::File::open(path).map_err(ToolError::Io)?;
    let mut body = String::new();
    let mut used = 0;
    let mut total = 0;
    let mut last = start;
    let mut next = None;
    // Scan without keeping the entire file in memory. In particular, there is
    // no 100 KB prefix cutoff: line 17,000 is as addressable as line 1.
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(ToolError::Io)?;
        total = index + 1;
        if total < start || total > end || next.is_some() {
            continue;
        }
        let offset = if total == start { column - 1 } else { 0 };
        let length = line.chars().count();
        if offset > length {
            return Err(ToolError::InvalidArgs(format!(
                "start_column exceeds line {start}'s length ({length} characters)"
            )));
        }
        let prefix = format!("{total}: ");
        let room = CHUNK_CHARS - used;
        if room <= prefix.len() + 1 {
            next = Some((total, offset + 1));
            continue;
        }
        let take = (room - prefix.len() - 1).min(length - offset);
        body.push_str(&prefix);
        body.extend(line.chars().skip(offset).take(take));
        body.push('\n');
        used += prefix.len() + take + 1;
        last = total;
        if take < length - offset {
            next = Some((total, offset + take + 1));
        }
    }
    if total == 0 {
        return Ok(ToolResult::ok("[Empty file: 0 lines; EOF]".into()));
    }
    if start > total {
        return Ok(ToolResult::ok(format!(
            "[EOF: file has {total} lines; start_line {start} is past the end]"
        )));
    }
    if next.is_none() && last < total {
        next = Some((last + 1, 1));
    }
    let continuation = match next {
        Some((line, col)) => format!("More file content available. To continue, call read_file with the same path, start_line={line}, start_column={col}; choose end_line as needed. To locate a particular symbol, use search_text on this file and jump to its matching lines instead of paging sequentially."),
        None => "EOF: no further content.".into(),
    };
    Ok(ToolResult::ok(format!("[Lines {start}-{last} of {total}; starting column {column}. Line numbers are labels, not file content.]\n{body}[{continuation}]")))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0.join("source.cpp"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    fn fixture(text: &str) -> (Fixture, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("companion-file-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("source.cpp");
        std::fs::write(&path, text).unwrap();
        (Fixture(dir), path)
    }
    #[test]
    fn reads_any_range_in_a_large_source_file() {
        let text = (1..=18_000)
            .map(|i| format!("source line {i}\n"))
            .collect::<String>();
        let (_dir, path) = fixture(&text);
        let result = read(
            &path,
            &serde_json::json!({"start_line":17000,"end_line":17004}),
        )
        .unwrap()
        .output;
        assert!(result.contains("Lines 17000-17004 of 18000"));
        assert!(result.contains("17004: source line 17004"));
        assert!(!result.contains("16999: source"));
        assert!(result.contains("start_line=17005"));
        let tail = read(&path, &serde_json::json!({"start_line":18000}))
            .unwrap()
            .output;
        assert!(tail.contains("18000: source line 18000"));
        assert!(tail.contains("EOF: no further content"));
    }
    #[test]
    fn chunks_are_bounded_and_long_unicode_lines_can_be_continued() {
        let (_dir, path) = fixture(&format!("{}TAIL\r\nsecond\n", "界".repeat(15_000)));
        let first = read(&path, &serde_json::json!({})).unwrap().output;
        assert!(first.chars().count() < 13_000);
        assert!(first.contains("start_line=1, start_column=11997"));
        let second = read(
            &path,
            &serde_json::json!({"start_line":1,"start_column":11997}),
        )
        .unwrap()
        .output;
        assert!(second.contains("TAIL\n2: second"));
        assert!(second.contains("EOF"));
    }
    #[test]
    fn validates_ranges_and_reports_empty_or_past_eof() {
        let (_dir, path) = fixture("one\ntwo\n");
        for args in [
            serde_json::json!({"start_line":0}),
            serde_json::json!({"start_line":-1}),
            serde_json::json!({"start_line":2,"end_line":1}),
            serde_json::json!({"start_line":"2"}),
            serde_json::json!({"start_column":10}),
        ] {
            assert!(read(&path, &args).is_err());
        }
        assert!(read(&path, &serde_json::json!({"start_line":3}))
            .unwrap()
            .output
            .contains("past the end"));
        std::fs::write(&path, "").unwrap();
        assert!(read(&path, &serde_json::json!({}))
            .unwrap()
            .output
            .contains("Empty file"));
    }
}
