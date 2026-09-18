//! Terminal execution (§32): timeouts, captured stdout/stderr/exit code.
//!
//! Runs inside the workspace directory. Approval is enforced by the caller
//! (Stage 9 gate); this module enforces resource limits independently:
//! wall-clock timeout with kill, output truncation, and a catastrophic-pattern
//! denylist. Real sandboxing (containers/OS sandboxes) is Stage 14.
//!
//! Output is collected as it arrives and a timeout kills the whole process
//! tree. Killing the shell alone left the real program running with the pipe
//! open, so every timed-out command returned nothing at all: a model watched
//! a dev server come back "exit 1, stdout (empty)" and concluded it was up
//! (night decision 61). A command that is meant to keep running belongs in
//! the background instead — `start_background` keeps it alive with its output
//! collected for later reads.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub const MAX_TIMEOUT_SECS: u64 = 1800; // 30 minutes (§32)
pub const MAX_OUTPUT_CHARS: usize = 50_000;
/// Characters kept per stream of a background command: a dev server prints
/// steadily for as long as it runs, and only the recent lines say anything.
const BACKGROUND_OUTPUT_CHARS: usize = 60_000;

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Substrings that are never executed, whatever the approval state.
/// This is defense-in-depth only — explicit user approval is the real gate.
fn denied(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_lowercase();
    for pat in [
        "rm -rf /",
        "rm -rf /*",
        "mkfs",
        ":(){ :|:& };:",
        "format c:",
        "del /s /q c:\\windows",
        "rd /s /q c:\\windows",
        // Stage 36–37 sandbox policy: destructive git must go through the
        // explicit Git confirm flow, never a raw shell string.
        "git reset --hard",
        "git push --force",
        "git push -f",
        "git clean -fd",
        "git clean -fdx",
    ] {
        if lower.contains(pat) {
            return Some(pat);
        }
    }
    None
}

/// Commands whose whole purpose is not to exit: dev servers, static servers,
/// watchers. Run in the foreground they hold the run still until the timeout
/// and then report that they were killed - two minutes of nothing, twice in a
/// row in one live run (owner watching, 2026-09-18), when the model had
/// already started the same server correctly in the background a step earlier.
pub fn keeps_running(command: &str) -> Option<&'static str> {
    let text = command.to_lowercase();
    const SERVERS: [&str; 18] = [
        "npm run dev",
        "npm start",
        "yarn dev",
        "pnpm dev",
        "npx serve",
        "npx vite",
        "vite dev",
        "next dev",
        "nuxt dev",
        "ng serve",
        "webpack serve",
        "nodemon",
        "http.server",
        "flask run",
        "uvicorn ",
        "rails server",
        "php -s",
        "jekyll serve",
    ];
    if let Some(found) = SERVERS.iter().find(|server| text.contains(*server)) {
        return Some(found);
    }
    // `serve .` or `vite` on their own, and anything watching for changes.
    // Matched as whole words: `grep -w` and `watch.py` are not servers.
    let words: Vec<&str> = text.split_whitespace().collect();
    for word in ["serve", "vite", "watch", "--watch", "-watch"] {
        if words.contains(&word) {
            return Some(word);
        }
    }
    None
}

fn shell() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

/// Start the command with both streams piped and no stdin: a program that
/// asks a question would otherwise wait for an answer nobody can give.
fn spawn(cmd: &str, cwd: &Path) -> Result<Child, String> {
    let (shell, flag) = shell();
    let mut command = Command::new(shell);
    command
        .arg(flag)
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so the whole tree can be ended at once.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map_err(|e| format!("cannot spawn shell: {e}"))
}

/// Read one stream into a buffer the caller can read at any time, including
/// while the command still runs and after it is killed.
fn collect<R: Read + Send + 'static>(pipe: Option<R>, cap: usize) -> Arc<Mutex<String>> {
    let buffer = Arc::new(Mutex::new(String::new()));
    let Some(mut pipe) = pipe else {
        return buffer;
    };
    let target = buffer.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let text = String::from_utf8_lossy(&chunk[..read]).into_owned();
                    if let Ok(mut held) = target.lock() {
                        held.push_str(&text);
                        // Keep the end: that is where a failure explains itself.
                        let length = held.chars().count();
                        if length > cap {
                            *held = held.chars().skip(length - cap).collect();
                        }
                    }
                }
            }
        }
    });
    buffer
}

fn read_buffer(buffer: &Arc<Mutex<String>>) -> String {
    buffer.lock().map(|held| held.clone()).unwrap_or_default()
}

/// End the command and everything it started. A shell exits without taking
/// its children with it, and on Windows a killed shell leaves the program it
/// launched running with the output pipe open.
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        // A negative pid is the process group started in `spawn`.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn run(cmd: &str, cwd: &Path, timeout_secs: u64) -> Result<CommandResult, String> {
    if cmd.trim().is_empty() {
        return Err("command is empty".into());
    }
    if let Some(pat) = denied(cmd) {
        return Err(format!(
            "refused catastrophic pattern {pat:?}; narrow the command"
        ));
    }
    let timeout = Duration::from_secs(timeout_secs.clamp(1, MAX_TIMEOUT_SECS));
    let mut child = spawn(cmd, cwd)?;
    let stdout = collect(child.stdout.take(), MAX_OUTPUT_CHARS * 2);
    let stderr = collect(child.stderr.take(), MAX_OUTPUT_CHARS * 2);

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            Some(st) => break Some(st),
            None => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    kill_tree(&mut child);
                    break child.try_wait().ok().flatten();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    // Let the readers drain what the pipes still hold.
    std::thread::sleep(Duration::from_millis(120));
    Ok(CommandResult {
        exit_code: status.and_then(|s| s.code()),
        stdout: head_tail(&read_buffer(&stdout), MAX_OUTPUT_CHARS),
        stderr: head_tail(&read_buffer(&stderr), MAX_OUTPUT_CHARS),
        timed_out,
    })
}

/// A command left running: a dev server, a watcher, anything whose whole
/// purpose is not to exit.
struct Background {
    command: String,
    owner: String,
    started: Instant,
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

#[derive(Debug, Clone)]
pub struct BackgroundStatus {
    pub id: u32,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub seconds: u64,
    pub stdout: String,
    pub stderr: String,
}

fn background() -> &'static Mutex<std::collections::HashMap<u32, Background>> {
    static RUNNING: OnceLock<Mutex<std::collections::HashMap<u32, Background>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn next_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn status_of(id: u32, entry: &mut Background) -> BackgroundStatus {
    let finished = entry.child.try_wait().ok().flatten();
    BackgroundStatus {
        id,
        command: entry.command.clone(),
        running: finished.is_none(),
        exit_code: finished.and_then(|s| s.code()),
        seconds: entry.started.elapsed().as_secs(),
        stdout: head_tail(&read_buffer(&entry.stdout), MAX_OUTPUT_CHARS),
        stderr: head_tail(&read_buffer(&entry.stderr), MAX_OUTPUT_CHARS),
    }
}

/// Start a command and leave it running. `settle` is how long to watch it
/// before reporting: long enough for a server to print its address or for a
/// broken command to fail, short enough not to hold up the caller.
pub fn start_background(
    cmd: &str,
    cwd: &Path,
    owner: &str,
    settle: Duration,
) -> Result<BackgroundStatus, String> {
    if cmd.trim().is_empty() {
        return Err("command is empty".into());
    }
    if let Some(pat) = denied(cmd) {
        return Err(format!(
            "refused catastrophic pattern {pat:?}; narrow the command"
        ));
    }
    // The same server, started again, is almost always a model that forgot it
    // already has one: itanswers with the one that is up rather than binding
    // another port (measured live, 2026-09-18: four servers for one folder).
    if let Ok(mut held) = background().lock() {
        let same: Vec<u32> = held
            .iter()
            .filter(|(_, entry)| entry.command == cmd && entry.owner == owner)
            .map(|(id, _)| *id)
            .collect();
        for id in same {
            let still_up = held
                .get_mut(&id)
                .map(|entry| entry.child.try_wait().ok().flatten().is_none())
                .unwrap_or(false);
            if still_up {
                if let Some(entry) = held.get_mut(&id) {
                    return Ok(status_of(id, entry));
                }
            }
        }
    }
    let mut child = spawn(cmd, cwd)?;
    let stdout = collect(child.stdout.take(), BACKGROUND_OUTPUT_CHARS);
    let stderr = collect(child.stderr.take(), BACKGROUND_OUTPUT_CHARS);
    let id = next_id();
    let mut entry = Background {
        command: cmd.to_string(),
        owner: owner.to_string(),
        started: Instant::now(),
        child,
        stdout,
        stderr,
    };
    let deadline = Instant::now() + settle;
    while Instant::now() < deadline {
        if entry.child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = status_of(id, &mut entry);
    if status.running {
        background()
            .lock()
            .map_err(|_| "background registry unavailable")?
            .insert(id, entry);
    }
    Ok(status)
}

/// What a background command has printed so far, and whether it is still up.
pub fn background_status(id: u32) -> Option<BackgroundStatus> {
    let mut held = background().lock().ok()?;
    let entry = held.get_mut(&id)?;
    Some(status_of(id, entry))
}

pub fn stop_background(id: u32) -> Option<BackgroundStatus> {
    let mut held = background().lock().ok()?;
    let mut entry = held.remove(&id)?;
    kill_tree(&mut entry.child);
    let mut status = status_of(id, &mut entry);
    status.running = false;
    Some(status)
}

/// Every background command started by one run, ended when it finishes: a
/// forgotten dev server holds its port and its memory against the next one.
pub fn stop_background_for(owner: &str) -> usize {
    let Ok(mut held) = background().lock() else {
        return 0;
    };
    let ids: Vec<u32> = held
        .iter()
        .filter(|(_, entry)| entry.owner == owner)
        .map(|(id, _)| *id)
        .collect();
    for id in &ids {
        if let Some(mut entry) = held.remove(id) {
            kill_tree(&mut entry.child);
        }
    }
    ids.len()
}

pub fn background_commands() -> Vec<(u32, String)> {
    background()
        .lock()
        .map(|held| {
            let mut all: Vec<(u32, String)> = held
                .iter()
                .map(|(id, entry)| (*id, entry.command.clone()))
                .collect();
            all.sort_by_key(|(id, _)| *id);
            all
        })
        .unwrap_or_default()
}

/// At most `limit` characters of `text`: a third from the start and two
/// thirds from the end, joined by a line saying how much was left out. Build
/// and test output puts its errors last, which a cut from the start dropped.
pub fn head_tail(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let keep = limit.saturating_sub(80.min(limit / 4));
    let head_chars = keep / 3;
    let tail_chars = keep - head_chars;
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text.chars().skip(total - tail_chars).collect();
    format!(
        "{head}\n[... {} characters omitted ...]\n{tail}",
        total - head_chars - tail_chars
    )
}

/// The shell's own way to do what a command it does not have would have done.
/// Two steps of one run went on Unix pipes against a shell without them
/// (night decision 61); naming the shell in the instructions was not enough.
pub fn shell_hint(stderr: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let marker = "is not recognized as an internal or external command";
    let line = stderr.lines().find(|line| line.contains(marker))?;
    let quote = '\'';
    let name = line.split(quote).nth(1).unwrap_or("").trim().to_lowercase();
    let equivalent = match name.as_str() {
        "ls" => Some("`dir`"),
        "cat" => Some("`type`"),
        "rm" => Some("`del`, or `rd /s /q` for a folder"),
        "cp" => Some("`copy`"),
        "mv" => Some("`move`"),
        "grep" => Some("`findstr`"),
        "which" => Some("`where`"),
        "pwd" => Some("`cd`"),
        "touch" => Some("`type nul > file`"),
        "head" | "tail" | "sed" | "awk" | "wc" | "jq" => Some(
            "nothing: run the command without it, or put the whole thing through `powershell -NoProfile -Command \"...\"`",
        ),
        _ => None,
    };
    let named = if name.is_empty() {
        "that command".to_string()
    } else {
        format!("`{name}`")
    };
    Some(match equivalent {
        Some(equivalent) => format!(
            "\n[This shell is cmd.exe and has no {named}. Use {equivalent}. Unix pipelines are not available here: send the command again without them.]"
        ),
        None => format!(
            "\n[This shell is cmd.exe and has no {named}. Its own commands are dir, type, copy, move, del, findstr and where; anything else goes through `powershell -NoProfile -Command \"...\"`.]"
        ),
    })
}

pub fn format_result(cmd: &str, r: &CommandResult) -> String {
    let mut out = format!(
        "Command:\n{cmd}\n\nExit code:\n{}\n",
        r.exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "killed".into())
    );
    if r.timed_out {
        out.push_str(
            "\n(still running at the timeout and stopped; what it printed until then is below. A command meant to keep running, such as a dev server, belongs in the background: add \"background\": true.)\n",
        );
    }
    let section = |name: &str, text: &str| {
        format!(
            "\n--- {name} ---\n{}",
            if text.is_empty() { "(empty)" } else { text }
        )
    };
    // A failed command's explanation is usually on stderr: it comes first, so
    // a later cut to the model's room can never remove it in favour of stdout.
    if r.timed_out || r.exit_code != Some(0) {
        out.push_str(&section("stderr", &r.stderr));
        out.push('\n');
        out.push_str(&section("stdout", &r.stdout));
    } else {
        out.push_str(&section("stdout", &r.stdout));
        out.push('\n');
        out.push_str(&section("stderr", &r.stderr));
    }
    if let Some(hint) = shell_hint(&r.stderr) {
        out.push_str(&hint);
    }
    out
}

pub fn format_background(status: &BackgroundStatus) -> String {
    let state = if status.running {
        "still running".to_string()
    } else {
        format!(
            "exited with code {}",
            status
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".into())
        )
    };
    let mut out = format!(
        "Command (background):\n{}\n\nId {}, {}, {} s so far\n",
        status.command, status.id, state, status.seconds,
    );
    if status.running {
        out.push_str(&format!(
            "Read its later output with execute_command {{\"background_id\": {id}}}; stop it with {{\"background_id\": {id}, \"stop\": true}}. It is stopped for you when this task ends.\n",
            id = status.id
        ));
    }
    let section = |name: &str, text: &str| {
        format!(
            "\n--- {name} ---\n{}",
            if text.is_empty() { "(empty)" } else { text }
        )
    };
    out.push_str(&section("stdout", &status.stdout));
    out.push('\n');
    out.push_str(&section("stderr", &status.stderr));
    if let Some(hint) = shell_hint(&status.stderr) {
        out.push_str(&hint);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_keeps_its_end_where_the_errors_are() {
        let output = format!(
            "{}error[E0308]: mismatched types at src/main.rs:12",
            "compiling crate\n".repeat(4_000)
        );
        let kept = head_tail(&output, 16_000);
        assert!(kept.chars().count() <= 16_000);
        assert!(kept.starts_with("compiling crate"));
        assert!(kept.ends_with("error[E0308]: mismatched types at src/main.rs:12"));
        assert!(kept.contains("characters omitted"));
        assert_eq!(head_tail("short", 16_000), "short");
    }

    #[test]
    fn a_failed_command_shows_stderr_first() {
        let failed = CommandResult {
            exit_code: Some(1),
            stdout: "x".repeat(60_000),
            stderr: "error: boom".into(),
            timed_out: false,
        };
        let text = format_result("cargo build", &failed);
        assert!(text.find("--- stderr ---").unwrap() < text.find("--- stdout ---").unwrap());
        let ok = CommandResult {
            exit_code: Some(0),
            stdout: "done".into(),
            stderr: String::new(),
            timed_out: false,
        };
        let text = format_result("cargo build", &ok);
        assert!(text.find("--- stdout ---").unwrap() < text.find("--- stderr ---").unwrap());
    }

    #[test]
    fn echo_captures_stdout_and_zero_exit() {
        let dir = std::env::temp_dir();
        let r = run("echo hello-terminal", &dir, 10).unwrap();
        assert!(!r.timed_out);
        assert_eq!(r.exit_code, Some(0));
        assert!(r.stdout.contains("hello-terminal"), "{:?}", r.stdout);
    }

    #[test]
    fn nonzero_exit_reported() {
        let dir = std::env::temp_dir();
        let r = run("exit 3", &dir, 10).unwrap();
        assert!(!r.timed_out);
        assert_eq!(r.exit_code, Some(3));
    }

    #[test]
    fn timeout_kills_long_command() {
        let dir = std::env::temp_dir();
        #[cfg(windows)]
        let cmd = "ping -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let cmd = "sleep 30";
        let start = Instant::now();
        let r = run(cmd, &dir, 2).unwrap();
        assert!(r.timed_out, "{r:?}");
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "kill took too long"
        );
    }

    #[test]
    fn a_timed_out_command_still_returns_what_it_printed() {
        let dir = std::env::temp_dir();
        #[cfg(windows)]
        let cmd = "echo started-and-waiting & ping -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let cmd = "echo started-and-waiting; sleep 30";
        let r = run(cmd, &dir, 2).unwrap();
        assert!(r.timed_out);
        assert!(
            r.stdout.contains("started-and-waiting"),
            "output printed before the timeout must survive the kill: {:?}",
            r.stdout
        );
        assert!(format_result(cmd, &r).contains("belongs in the background"));
    }

    #[test]
    fn a_command_that_never_exits_is_recognised_before_it_is_run() {
        for command in [
            "npm run dev",
            "cd ORBITAL && npx serve .",
            "npx serve . --no-clipboard",
            "python -m http.server 8000",
            "npm start",
            "uvicorn app:main --reload",
            "cargo watch -x test",
            "vite",
            "tsc --watch",
        ] {
            assert!(keeps_running(command).is_some(), "{command} does not exit on its own");
        }
        for command in [
            "npm run build",
            "npm install",
            "python -m unittest discover -q",
            "cargo test --quiet",
            "git status",
            "dir",
            "npm run test",
        ] {
            assert!(keeps_running(command).is_none(), "{command} finishes by itself");
        }
    }

    #[test]
    fn a_background_command_keeps_running_and_can_be_read_then_stopped() {
        let dir = std::env::temp_dir();
        #[cfg(windows)]
        let cmd = "echo server-listening & ping -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let cmd = "echo server-listening; sleep 30";
        let started =
            start_background(cmd, &dir, "test-owner", Duration::from_millis(1500)).unwrap();
        assert!(
            started.running,
            "a server command must not be reported as finished"
        );
        assert!(
            started.stdout.contains("server-listening"),
            "{:?}",
            started.stdout
        );
        assert!(format_background(&started).contains("background_id"));
        let later = background_status(started.id).expect("still registered");
        assert!(later.running);
        // Asking for the same server again gives back the one already running.
        let again = start_background(cmd, &dir, "test-owner", Duration::from_millis(100)).unwrap();
        assert_eq!(again.id, started.id, "one server, not two");

        let stopped = stop_background(started.id).expect("stoppable");
        assert!(!stopped.running);
        assert!(background_status(started.id).is_none());
        assert_eq!(stop_background_for("test-owner"), 0);
    }

    #[test]
    fn background_commands_of_one_run_are_ended_together() {
        let dir = std::env::temp_dir();
        // Two different commands: the same one twice is deliberately given back
        // the process that is already up, which the test above covers.
        #[cfg(windows)]
        let (one, two) = ("ping -n 30 127.0.0.1 >nul", "ping -n 29 127.0.0.1 >nul");
        #[cfg(not(windows))]
        let (one, two) = ("sleep 30", "sleep 29");
        let first = start_background(one, &dir, "run-42", Duration::from_millis(300)).unwrap();
        let second = start_background(two, &dir, "run-42", Duration::from_millis(300)).unwrap();
        assert_eq!(stop_background_for("run-42"), 2);
        assert!(background_status(first.id).is_none());
        assert!(background_status(second.id).is_none());
    }

    #[test]
    fn an_unknown_command_is_answered_with_this_shells_own() {
        let windows_error =
            "'tail' is not recognized as an internal or external command,\noperable program or batch file.";
        let hint = shell_hint(windows_error);
        if cfg!(windows) {
            let hint = hint.expect("a hint on Windows");
            assert!(hint.contains("cmd.exe"), "{hint}");
            assert!(hint.contains("powershell"), "{hint}");
            assert!(shell_hint("'ls' is not recognized as an internal or external command")
                .unwrap()
                .contains("`dir`"));
            assert!(shell_hint("error: nothing to do").is_none());
        } else {
            assert!(hint.is_none(), "the hint names cmd.exe and belongs to it");
        }
    }

    #[test]
    fn catastrophic_patterns_refused() {
        let dir = std::env::temp_dir();
        assert!(run("rm -rf / --no-preserve-root", &dir, 5).is_err());
        assert!(run("git reset --hard HEAD", &dir, 5).is_err());
        assert!(run("git push --force origin main", &dir, 5).is_err());
        assert!(run("git status", &dir, 5).is_ok());
        assert!(run("echo safe", &dir, 5).is_ok());
        assert!(start_background("rm -rf /", &dir, "test", Duration::from_millis(10)).is_err());
    }

    #[test]
    fn empty_command_rejected() {
        assert!(run("   ", std::env::temp_dir().as_path(), 5).is_err());
    }
}
