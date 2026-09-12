//! Terminal execution (§32): timeouts, captured stdout/stderr/exit code.
//!
//! Runs inside the workspace directory. Approval is enforced by the caller
//! (Stage 9 gate); this module enforces resource limits independently:
//! wall-clock timeout with kill, output truncation, and a catastrophic-pattern
//! denylist. Real sandboxing (containers/OS sandboxes) is Stage 14.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub const MAX_TIMEOUT_SECS: u64 = 1800; // 30 minutes (§32)
pub const MAX_OUTPUT_CHARS: usize = 50_000;

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
    let (shell, flag): (&str, &str) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut child = Command::new(shell)
        .arg(flag)
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot spawn shell: {e}"))?;

    let mut out_rx = None;
    let mut err_rx = None;
    if let Some(pipe) = child.stdout.take() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = pipe.take(u64::MAX).read_to_string(&mut s);
            let _ = tx.send(s);
        });
        out_rx = Some(rx);
    }
    if let Some(pipe) = child.stderr.take() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = pipe.take(u64::MAX).read_to_string(&mut s);
            let _ = tx.send(s);
        });
        err_rx = Some(rx);
    }

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            Some(st) => break Some(st),
            None => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    // Pipes hit EOF once the process (and its children, on kill) exits.
    let stdout: String = out_rx
        .and_then(|rx| rx.recv_timeout(Duration::from_secs(5)).ok())
        .unwrap_or_default();
    let stderr: String = err_rx
        .and_then(|rx| rx.recv_timeout(Duration::from_secs(5)).ok())
        .unwrap_or_default();
    Ok(CommandResult {
        exit_code: status.and_then(|s| s.code()),
        stdout: stdout.chars().take(MAX_OUTPUT_CHARS).collect(),
        stderr: stderr.chars().take(MAX_OUTPUT_CHARS).collect(),
        timed_out,
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
        out.push_str("\n(timed out and was killed)\n");
    }
    out.push_str("\n--- stdout ---\n");
    out.push_str(if r.stdout.is_empty() {
        "(empty)"
    } else {
        &r.stdout
    });
    out.push_str("\n\n--- stderr ---\n");
    out.push_str(if r.stderr.is_empty() {
        "(empty)"
    } else {
        &r.stderr
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        #[cfg(windows)]
        let r = run("exit 3", &dir, 10).unwrap();
        #[cfg(not(windows))]
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
    fn catastrophic_patterns_refused() {
        let dir = std::env::temp_dir();
        assert!(run("rm -rf / --no-preserve-root", &dir, 5).is_err());
        assert!(run("git reset --hard HEAD", &dir, 5).is_err());
        assert!(run("git push --force origin main", &dir, 5).is_err());
        assert!(run("git status", &dir, 5).is_ok());
        assert!(run("echo safe", &dir, 5).is_ok());
    }

    #[test]
    fn empty_command_rejected() {
        assert!(run("   ", std::env::temp_dir().as_path(), 5).is_err());
    }
}
