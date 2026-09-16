//! A bounded in-memory tail of the application's own log.
//!
//! The log went only to the console, so the only way to carry a failure off a
//! machine was to photograph the terminal. Keeping the recent lines in memory
//! lets a session export carry them, which is how a run on one machine gets
//! read on another.
//!
//! Bounded on purpose: this is a tail for diagnosis, not an audit trail, and
//! it must never grow without limit in a long-running process.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Lines kept. At a typical line length this is a few hundred kilobytes, and
/// covers a long agent run with room to spare.
const CAPACITY: usize = 2_000;

/// The tail this process writes to. A process-wide log belongs to the
/// process, not to one request handler, so it is reached directly rather than
/// threaded through every constructor.
pub fn global() -> &'static LogTail {
    static TAIL: std::sync::OnceLock<LogTail> = std::sync::OnceLock::new();
    TAIL.get_or_init(LogTail::new)
}

#[derive(Clone, Default)]
pub struct LogTail {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogTail {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(CAPACITY))),
        }
    }

    fn push(&self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        if let Ok(mut lines) = self.lines.lock() {
            if lines.len() == CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.to_string());
        }
    }

    /// The most recent `limit` lines, oldest first.
    pub fn recent(&self, limit: usize) -> Vec<String> {
        let Ok(lines) = self.lines.lock() else {
            return Vec::new();
        };
        lines
            .iter()
            .skip(lines.len().saturating_sub(limit))
            .cloned()
            .collect()
    }
}

/// Writer half handed to the tracing layer. Formatted records arrive here as
/// bytes; a record can span several writes, so lines are split on newlines
/// and anything without one is kept as its own entry rather than lost.
pub struct TailWriter(LogTail);

impl std::io::Write for TailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(text) = std::str::from_utf8(buf) {
            for line in text.lines() {
                self.0.push(line);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogTail {
    type Writer = TailWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TailWriter(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn the_tail_keeps_the_newest_lines_and_stays_bounded() {
        let tail = LogTail::new();
        for index in 0..CAPACITY + 50 {
            tail.push(&format!("line {index}"));
        }
        let recent = tail.recent(CAPACITY * 2);
        assert_eq!(recent.len(), CAPACITY);
        assert_eq!(recent.last().unwrap(), &format!("line {}", CAPACITY + 49));
        assert_eq!(recent.first().unwrap(), &format!("line {}", 50));
        assert_eq!(tail.recent(3).len(), 3);
    }

    #[test]
    fn a_multi_line_record_arrives_as_separate_lines_and_blanks_are_dropped() {
        let tail = LogTail::new();
        let mut writer = TailWriter(tail.clone());
        writer.write_all(b"first\n\nsecond\n").unwrap();
        assert_eq!(tail.recent(10), vec!["first".to_string(), "second".to_string()]);
    }
}
