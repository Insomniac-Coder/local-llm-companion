//! Build and test a project without being told how (owner request, 2026-09-18).
//!
//! A model that has to guess the command spends steps on it and misreads what
//! comes back: one run asked for `npm run build` in a folder with no TypeScript
//! configuration and got the compiler's *help text*, which says nothing about
//! the code (night decision 61). This finds what the project is, runs its own
//! build and tests, and reports the problems as `file:line: message` rather
//! than as a wall of output.

use std::path::Path;
use std::time::{Duration, Instant};

/// Problems listed before the rest are counted instead.
const MAX_PROBLEMS: usize = 20;
/// Output kept when nothing could be parsed out of it.
const TAIL_LINES: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What this proves: "build" or "tests".
    pub kind: &'static str,
    pub command: String,
    /// The program the command needs. A PC without it gets told so, rather
    /// than a shell error nobody can act on (owner question, 2026-09-18).
    pub program: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// What the project is, in the words of the file that said so.
    pub kind: String,
    pub steps: Vec<Step>,
}

/// Whether this PC has the program, looked for where the shell looks. On
/// Windows the launchers are `.exe`, `.cmd` or `.bat`; npm is a `.cmd`.
pub fn on_path(program: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|ext| format!("{program}.{ext}")).collect()
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(&path).any(|directory| names.iter().any(|name| directory.join(name).is_file()))
}

/// The Python this PC actually has, under whichever of its names.
fn python_program() -> Option<&'static str> {
    ["python", "python3", "py"].into_iter().find(|name| on_path(name))
}

fn script(scripts: &serde_json::Value, name: &str) -> bool {
    scripts
        .get(name)
        .and_then(|value| value.as_str())
        .is_some_and(|text| !text.trim().is_empty() && !text.contains("no test specified"))
}

/// What to run for the project in this folder, or nothing when it is not a
/// project this knows how to check.
pub fn plan(root: &Path) -> Option<Plan> {
    if let Ok(text) = std::fs::read_to_string(root.join("package.json")) {
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let scripts = manifest.get("scripts").cloned().unwrap_or_default();
        let mut steps = Vec::new();
        for (kind, name) in [("build", "build"), ("build", "typecheck"), ("tests", "test")] {
            if script(&scripts, name) && !steps.iter().any(|step: &Step| step.kind == kind) {
                steps.push(Step { kind, command: format!("npm run {name}"), program: "npm".into() });
            }
        }
        if steps.is_empty() {
            return None;
        }
        return Some(Plan { kind: "npm (package.json)".into(), steps });
    }
    if root.join("Cargo.toml").is_file() {
        return Some(Plan {
            kind: "cargo (Cargo.toml)".into(),
            steps: vec![
                Step { kind: "build", command: "cargo check --message-format short".into(), program: "cargo".into() },
                Step { kind: "tests", command: "cargo test --quiet".into(), program: "cargo".into() },
            ],
        });
    }
    if root.join("go.mod").is_file() {
        return Some(Plan {
            kind: "go (go.mod)".into(),
            steps: vec![
                Step { kind: "build", command: "go build ./...".into(), program: "go".into() },
                Step { kind: "tests", command: "go test ./...".into(), program: "go".into() },
            ],
        });
    }
    let python = root.join("pyproject.toml").is_file()
        || root.join("setup.py").is_file()
        || root.join("requirements.txt").is_file()
        || root.join("tests").is_dir();
    if python {
        let pytest = root.join("pytest.ini").is_file()
            || std::fs::read_to_string(root.join("pyproject.toml"))
                .map(|text| text.contains("pytest"))
                .unwrap_or(false);
        return Some(Plan {
            kind: "python".into(),
            steps: vec![Step {
                kind: "tests",
                command: python_tests(root, pytest),
                program: python_program().unwrap_or("python").into(),
            }],
        });
    }
    None
}

/// How to run this project's Python tests.
///
/// The common layout - a `tests/` folder with no `__init__.py`, importing the
/// code beside it - defeats plain discovery in both directions: started from
/// the root it finds nothing (exit 5), and started at `tests` it cannot import
/// the folder. Discovering *inside* `tests` with the project root on the import
/// path is the form that works, and it is what a person would type.
fn python_tests(root: &Path, pytest: bool) -> String {
    let python = python_program().unwrap_or("python");
    if pytest {
        return format!("{python} -m pytest -q");
    }
    let tests = root.join("tests");
    if !tests.is_dir() {
        return format!("{python} -m unittest discover -q");
    }
    if tests.join("__init__.py").is_file() {
        return format!("{python} -m unittest discover -q -s tests -t .");
    }
    if cfg!(windows) {
        format!("set PYTHONPATH=. && {python} -m unittest discover -q -s tests -t tests")
    } else {
        format!("PYTHONPATH=. {python} -m unittest discover -q -s tests -t tests")
    }
}

/// One problem line, as the compiler or test runner reported it. Whole lines
/// are kept: a model needs the message, not a classification of it.
pub fn problems(output: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        let looks_like_a_problem = lower.contains("error")
            || lower.starts_with("failed")
            || lower.starts_with("failures:")
            || lower.starts_with("e   ")
            || line.starts_with("FAIL")
            || lower.contains("panicked at")
            || lower.starts_with("assert");
        if !looks_like_a_problem {
            continue;
        }
        // Noise that repeats the run rather than describing a fault.
        if lower.starts_with("npm error a complete log")
            || lower.contains("error: could not compile")
            || lower.starts_with("error: test failed")
            || lower.contains("--nocapture")
        {
            continue;
        }
        let mut entry: String = line.chars().take(220).collect();
        // Rust puts the place on the next line: `--> src/main.rs:12:5`.
        if let Some(next) = lines.get(index + 1) {
            let next = next.trim();
            if next.starts_with("-->") && entry.len() + next.len() < 240 {
                entry.push_str(&format!(" ({})", next.trim_start_matches("--> ")));
            }
        }
        if !found.contains(&entry) {
            found.push(entry);
        }
    }
    found
}

fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines[start..].join("\n")
}

/// Run a project's own build and tests and report what failed. The build runs
/// first: there is nothing to learn from a test run that could not compile.
pub fn run(root: &Path, timeout_secs: u64) -> String {
    let Some(plan) = plan(root) else {
        return "No build or test command was found here: no package.json with a build or test script, no Cargo.toml, no go.mod, and no Python tests. Run the project's own command with execute_command, or say what should be run.".to_string();
    };
    let mut report = format!("Project: {}\n", plan.kind);
    for step in &plan.steps {
        if !on_path(&step.program) {
            report.push_str(&format!(
                "\n{} ({}): {} is not installed on this PC (it is not on PATH), so this cannot be checked here. Install it, or run the project's own command yourself with execute_command.\n",
                step.command, step.kind, step.program
            ));
            continue;
        }
        let started = Instant::now();
        let result = crate::terminal::run(&step.command, root, timeout_secs);
        let seconds = started.elapsed().as_secs_f32();
        match result {
            Err(error) => {
                report.push_str(&format!("\n{} ({}): could not run: {error}\n", step.command, step.kind));
                break;
            }
            Ok(outcome) => {
                let output = format!("{}\n{}", outcome.stdout, outcome.stderr);
                let ok = outcome.exit_code == Some(0) && !outcome.timed_out;
                report.push_str(&format!(
                    "\n{} ({}): {} in {seconds:.1} s\n",
                    step.command,
                    step.kind,
                    if outcome.timed_out {
                        format!("still running after {timeout_secs} s and stopped")
                    } else {
                        format!("exit code {}", outcome.exit_code.map(|code| code.to_string()).unwrap_or_else(|| "killed".into()))
                    }
                ));
                if ok {
                    report.push_str(&format!("{} passed.\n", if step.kind == "build" { "The build" } else { "The tests" }));
                    continue;
                }
                if outcome.exit_code == Some(5) {
                    report.push_str(
                        "The runner found no tests to run. Check where the tests live and how they are named, or run them yourself with execute_command.\n",
                    );
                    continue;
                }
                if output.contains("Start directory is not importable") {
                    report.push_str(
                        "The tests folder is not importable as a package: add an empty tests/__init__.py, install pytest, or run the tests yourself with execute_command.\n",
                    );
                    continue;
                }
                let problems = problems(&output);
                if problems.is_empty() {
                    report.push_str(&format!("No error line could be picked out. The end of the output:\n{}\n", tail(&output)));
                } else {
                    report.push_str(&format!("{} problem(s):\n", problems.len()));
                    for problem in problems.iter().take(MAX_PROBLEMS) {
                        report.push_str(&format!("- {problem}\n"));
                    }
                    if problems.len() > MAX_PROBLEMS {
                        report.push_str(&format!("- [{} more]\n", problems.len() - MAX_PROBLEMS));
                    }
                }
                // A failed build makes the test run meaningless.
                if step.kind == "build" {
                    report.push_str("Fix these before the tests can say anything.\n");
                    break;
                }
            }
        }
    }
    report
}

/// The timeout a check gets: builds are slow, and being cut off mid-build
/// teaches a model nothing.
pub fn default_timeout() -> Duration {
    Duration::from_secs(600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("companion-check-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_project_says_what_to_run() {
        let dir = temp("npm");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts": {"dev": "vite", "build": "tsc -b && vite build", "test": "echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        let npm = plan(&dir).expect("a project");
        assert_eq!(npm.kind, "npm (package.json)");
        assert_eq!(npm.steps.len(), 1, "the placeholder test script is not a test: {:?}", npm.steps);
        assert_eq!(npm.steps[0].command, "npm run build");

        std::fs::write(dir.join("package.json"), r#"{"scripts": {"test": "vitest run"}}"#).unwrap();
        let tests_only = plan(&dir).unwrap();
        assert_eq!(tests_only.steps[0], Step { kind: "tests", command: "npm run test".into(), program: "npm".into() });

        // A folder that is not a project says so instead of guessing.
        let empty = temp("empty");
        assert!(plan_is_none(&empty));
        let rust = temp("rust");
        std::fs::write(rust.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(plan(&rust).unwrap().steps.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan_is_none(dir: &std::path::Path) -> bool {
        plan(dir).is_none()
    }

    #[test]
    fn the_errors_are_picked_out_of_the_noise() {
        let typescript = "> app@0.0.0 build\n> tsc -b && vite build\n\n\
            src/data.ts(1,10): error TS1484: 'Discovery' is a type and must be imported using a type-only import.\n\
            src/App.tsx(68,37): error TS6133: 'star' is declared but its value is never read.\n\
            src/data.ts(1,10): error TS1484: 'Discovery' is a type and must be imported using a type-only import.\n";
        let found = problems(typescript);
        assert_eq!(found.len(), 2, "the same error twice is one problem: {found:?}");
        assert!(found[0].starts_with("src/data.ts(1,10): error TS1484"), "{found:?}");

        let rust = "error[E0308]: mismatched types\n  --> src/main.rs:12:5\n   |\nerror: could not compile `app` (bin \"app\") due to 1 previous error\n";
        let found = problems(rust);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("src/main.rs:12:5"), "the place travels with the message: {found:?}");

        // Nothing to pick out is not the same as nothing wrong.
        assert!(problems("Everything is fine\nbuilt in 1.2s").is_empty());
        assert_eq!(tail("a\n\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn python_tests_are_discovered_where_they_actually_live() {
        let dir = temp("python");
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(dir.join("orders.py"), "def total():\n    return 1\n").unwrap();
        std::fs::write(dir.join("tests/test_orders.py"), "import unittest\nfrom orders import total\n").unwrap();
        // The common layout: tests/ beside the code, with no package marker.
        let found = plan(&dir).expect("a python project");
        assert!(found.steps[0].command.contains("-s tests -t tests"), "{}", found.steps[0].command);
        assert!(found.steps[0].command.contains("PYTHONPATH"), "the code beside the tests must stay importable: {}", found.steps[0].command);

        // With a package marker the plain form works and is used.
        std::fs::write(dir.join("tests/__init__.py"), "").unwrap();
        assert_eq!(plan(&dir).unwrap().steps[0].command, "python -m unittest discover -q -s tests -t .");

        // pytest when the project says so.
        std::fs::write(dir.join("pyproject.toml"), "[tool.pytest.ini_options]\n").unwrap();
        assert_eq!(plan(&dir).unwrap().steps[0].command, "python -m pytest -q");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_that_found_no_tests_says_so_rather_than_no_errors() {
        let dir = temp("notests");
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(dir.join("tests/__init__.py"), "").unwrap();
        // A tests package with nothing in it: the runner exits 5, which is
        // neither a clean run nor a failure to compile.
        let report = run(&dir, 60);
        assert!(report.contains("found no tests to run"), "{report}");
        assert!(!report.contains("No error line could be picked out"), "{report}");

        // The same with no package marker: the form chosen for that layout
        // still runs, and still reports honestly that it found nothing.
        let bare = temp("notests-bare");
        std::fs::create_dir_all(bare.join("tests")).unwrap();
        let report = run(&bare, 60);
        assert!(report.contains("found no tests to run"), "{report}");
        assert!(!report.contains("not importable"), "the layout that used to fail now runs: {report}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn a_toolchain_this_pc_does_not_have_is_named_rather_than_run() {
        let dir = temp("no-toolchain");
        std::fs::write(dir.join("package.json"), r#"{"scripts": {"build": "webpack"}}"#).unwrap();
        let plan = plan(&dir).unwrap();
        assert_eq!(plan.steps[0].program, "npm");

        // Whatever this PC has or lacks, the report never leaves a missing
        // program looking like a fault in the code.
        let report = run(&dir, 20);
        if on_path("npm") {
            assert!(report.contains("npm run build"), "{report}");
        } else {
            assert!(report.contains("npm is not installed on this PC"), "{report}");
            assert!(report.contains("execute_command"), "{report}");
        }
        assert!(on_path(if cfg!(windows) { "cmd" } else { "sh" }), "the shell itself is always there");
        assert!(!on_path("a-program-this-pc-certainly-does-not-have"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_is_not_a_project_is_told_plainly() {
        let dir = temp("nothing");
        let report = run(&dir, 5);
        assert!(report.contains("No build or test command was found here"), "{report}");
        assert!(report.contains("execute_command"), "it says what to do instead: {report}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
