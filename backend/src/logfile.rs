//! The application's log and the model server's output, kept on disk.
//!
//! Both lived only in the console and in memory, so a restart erased them:
//! when a model server stopped in the middle of a long run and the PC was
//! restarted, nothing was left to say why (owner, 2026-09-18). They are
//! written to `logs/` in the data folder now. Each file is capped and rotated
//! by renaming: `companion.log` becomes `companion.1.log`, which takes the
//! place of the `companion.2.log` before it, so the folder stays at a few
//! files of a few megabytes each.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Size at which a file is rotated, and how many older files are kept.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const KEEP: usize = 2;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static APP: OnceLock<RotatingLog> = OnceLock::new();
static MODEL_SERVER: OnceLock<RotatingLog> = OnceLock::new();

/// Sends this process's logs to `dir` and marks where this start begins.
/// Records logged before this (the first lines of a start) reach only the
/// console and the in-memory tail.
pub fn init(dir: &Path) {
    if DIR.set(dir.to_path_buf()).is_err() {
        return;
    }
    if let Some(log) = app() {
        log.append(format!("\n===== Companion started {} =====\n", now()).as_bytes());
    }
}

/// `companion.log`: every record the application logs.
pub fn app() -> Option<&'static RotatingLog> {
    let dir = DIR.get()?;
    Some(APP.get_or_init(|| RotatingLog::new(dir.join("companion.log"), MAX_BYTES, KEEP)))
}

/// `model-server.log`: everything the model server prints, a line at a time
/// with the time it arrived, and what Companion did to it.
pub fn model_server() -> Option<&'static RotatingLog> {
    let dir = DIR.get()?;
    Some(MODEL_SERVER.get_or_init(|| RotatingLog::new(dir.join("model-server.log"), MAX_BYTES, KEEP)))
}

/// A line of Companion's own in the model server's log: started, ready,
/// stopped, exited.
pub fn model_server_note(text: &str) {
    if let Some(log) = model_server() {
        log.append(format!("{} [companion] {text}\n", now()).as_bytes());
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// One log file with a size cap. Writing never fails the caller: a log that
/// cannot be written is skipped, never a reason to stop the app.
pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Mutex<Option<(File, u64)>>,
}

impl RotatingLog {
    pub fn new(path: PathBuf, max_bytes: u64, keep: usize) -> Self {
        Self {
            path,
            max_bytes,
            keep,
            file: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, bytes: &[u8]) {
        let Ok(mut slot) = self.file.lock() else {
            return;
        };
        if slot.is_none() {
            *slot = self.open();
        }
        let full = slot
            .as_ref()
            .is_some_and(|(_, size)| *size > 0 && size + bytes.len() as u64 > self.max_bytes);
        if full {
            // Closed before it is renamed: Windows does not rename a file
            // that is open.
            *slot = None;
            self.rotate();
            *slot = self.open();
        }
        if let Some((file, size)) = slot.as_mut() {
            if file.write_all(bytes).is_ok() {
                *size += bytes.len() as u64;
            }
        }
    }

    fn open(&self) -> Option<(File, u64)> {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new().create(true).append(true).open(&self.path).ok()?;
        let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Some((file, size))
    }

    /// `name.log` -> `name.1.log` -> ... -> `name.{keep}.log`; the rename
    /// takes the place of the oldest file.
    fn rotate(&self) {
        for n in (1..self.keep).rev() {
            let _ = std::fs::rename(self.numbered(n), self.numbered(n + 1));
        }
        let _ = std::fs::rename(&self.path, self.numbered(1));
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("log");
        let name = match self.path.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("{stem}.{n}.{ext}"),
            None => format!("{stem}.{n}"),
        };
        self.path.with_file_name(name)
    }
}

/// The tracing layer's writer: each formatted record goes to
/// `companion.log` once `init` has run.
pub struct AppFileWriter;

impl Write for AppFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(log) = app() {
            log.append(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Splits output into lines and puts the time each arrived in front of it:
/// the model server's own lines carry no wall-clock time. A line cut between
/// two reads is held until its end arrives.
#[derive(Default)]
pub struct LineStamper {
    pending: Vec<u8>,
}

impl LineStamper {
    /// Longest line held while waiting for its end; a line redrawn with
    /// carriage returns and never ended is written as it is past this.
    const HOLD: usize = 64 * 1024;

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            stamp_into(&mut out, &line);
        }
        if self.pending.len() > Self::HOLD {
            let line = std::mem::take(&mut self.pending);
            stamp_into(&mut out, &line);
            out.push(b'\n');
        }
        out
    }

    /// What is left when the output ends without a final newline.
    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            stamp_into(&mut out, &line);
            out.push(b'\n');
        }
        out
    }
}

fn stamp_into(out: &mut Vec<u8>, line: &[u8]) {
    out.extend_from_slice(now().as_bytes());
    out.push(b' ');
    out.extend_from_slice(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("companion-logfile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_full_log_is_rotated_and_only_the_newest_files_are_kept() {
        let dir = scratch("rotate");
        let log = RotatingLog::new(dir.join("app.log"), 100, 2);
        for n in 0..10 {
            log.append(format!("record {n:02} {}\n", "x".repeat(30)).as_bytes());
        }
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        assert!(read("app.log").contains("record 09"), "the newest record is in the live file");
        assert!(!read("app.1.log").is_empty() && !read("app.2.log").is_empty());
        assert!(!dir.join("app.3.log").exists(), "no more than two older files");
        for name in ["app.log", "app.1.log", "app.2.log"] {
            assert!(std::fs::metadata(dir.join(name)).unwrap().len() <= 100, "{name} stays under its cap");
        }
        assert!(read("app.2.log").contains("record 0"), "the oldest file kept holds older records");
        assert!(!read("app.log").contains("record 00"), "the first records have rotated out of the live file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_reopened_after_a_restart_appends_to_what_is_there() {
        let dir = scratch("reopen");
        RotatingLog::new(dir.join("app.log"), 10_000, 2).append(b"before the restart\n");
        RotatingLog::new(dir.join("app.log"), 10_000, 2).append(b"after the restart\n");
        let text = std::fs::read_to_string(dir.join("app.log")).unwrap();
        assert_eq!(text, "before the restart\nafter the restart\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_is_stamped_a_whole_line_at_a_time() {
        let mut lines = LineStamper::default();
        let first = lines.push(b"slot launch: id 0\nprompt proc");
        let text = String::from_utf8(first).unwrap();
        assert_eq!(text.lines().count(), 1, "the half line waits for its end: {text}");
        assert!(text.ends_with(" slot launch: id 0\n"), "{text}");
        assert!(text.starts_with("20"), "a date comes first: {text}");
        let second = String::from_utf8(lines.push(b"essing progress\n")).unwrap();
        assert!(second.ends_with(" prompt processing progress\n"), "{second}");
        assert!(lines.finish().is_empty());
        let _ = lines.push(b"no newline at the end");
        let rest = String::from_utf8(lines.finish()).unwrap();
        assert!(rest.ends_with(" no newline at the end\n"), "{rest}");
    }
}
