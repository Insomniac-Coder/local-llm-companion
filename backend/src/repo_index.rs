//! Repository index (Stage 32, §96): lightweight local file/symbol map.
//!
//! The agent uses this to discover relevant code WITHOUT dumping the repo
//! into context (§95: tree → search → files → symbols → sections).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const MAX_FILES: usize = 5000;
pub const MAX_FILE_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub path: String, // workspace-relative, forward slashes
    pub size: u64,
    pub ext: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoIndex {
    pub files: Vec<IndexedFile>,
    pub truncated: bool,
}

fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "target"
            | "node_modules"
            | "build"
            | "dist"
            | "out"
            | "bin"
            | "obj"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
    )
}

/// Candidate symbol lines: `keyword rest-of-line` (capped per file).
fn extract_symbols(ext: &str, text: &str) -> Vec<String> {
    let kws: &[&str] = match ext {
        "rs" => &[
            "fn ",
            "struct ",
            "enum ",
            "trait ",
            "impl ",
            "mod ",
            "macro_rules!",
        ],
        "py" => &["def ", "class "],
        "js" | "ts" | "tsx" | "jsx" | "mjs" | "cjs" => &[
            "function ",
            "class ",
            "const ",
            "export function ",
            "export class ",
            "export const ",
            "export default ",
        ],
        "cpp" | "hpp" | "h" | "c" | "cc" | "cxx" => &[
            "class ",
            "struct ",
            "void ",
            "int ",
            "auto ",
            "template",
            "namespace ",
        ],
        "cs" => &[
            "class ",
            "void ",
            "public ",
            "private ",
            "protected ",
            "internal ",
            "namespace ",
        ],
        "go" => &["func ", "type "],
        "java" | "kt" => &[
            "class ",
            "interface ",
            "void ",
            "public ",
            "private ",
            "fun ",
        ],
        _ => &["fn ", "def ", "class ", "function "],
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.len() > 160 {
            continue;
        }
        for kw in kws {
            if t.starts_with(kw) {
                let sig: String = t.chars().take(100).collect();
                out.push(sig);
                break;
            }
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

pub fn build(root: &Path) -> RepoIndex {
    let mut idx = RepoIndex::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if idx.files.len() >= MAX_FILES {
            idx.truncated = true;
            break;
        }
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map(|e| e.flatten().collect())
            .unwrap_or_default();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.')
                && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && skip_dir(&name)
            {
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if skip_dir(&name) {
                    continue;
                }
                stack.push(e.path());
                continue;
            }
            let rel = e
                .path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or(name.clone());
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = Path::new(&name)
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_lowercase();
            let symbols = if size > 0
                && size <= MAX_FILE_BYTES
                && !matches!(
                    ext.as_str(),
                    "exe" | "dll" | "so" | "bin" | "png" | "jpg" | "gguf"
                ) {
                std::fs::read_to_string(e.path())
                    .map(|t| extract_symbols(&ext, &t))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            idx.files.push(IndexedFile {
                path: rel,
                size,
                ext,
                symbols,
            });
            if idx.files.len() >= MAX_FILES {
                idx.truncated = true;
                break;
            }
        }
    }
    idx
}

/// Ranked file lookup: substring hits on path/symbols score highest.
pub fn search<'a>(idx: &'a RepoIndex, query: &str, limit: usize) -> Vec<&'a IndexedFile> {
    let q = query.to_lowercase();
    let terms: Vec<&str> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| {
            t.len() > 2
                && !matches!(
                    *t,
                    "the"
                        | "this"
                        | "that"
                        | "these"
                        | "those"
                        | "can"
                        | "could"
                        | "would"
                        | "you"
                        | "your"
                        | "about"
                        | "more"
                        | "tell"
                        | "please"
                        | "what"
                        | "which"
                        | "where"
                        | "how"
                        | "are"
                        | "was"
                        | "were"
                        | "for"
                        | "and"
                        | "with"
                        | "from"
                        | "still"
                        | "project"
                        | "list"
                        | "find"
                )
        })
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i64, &IndexedFile)> = idx
        .files
        .iter()
        .map(|f| {
            let path = f.path.to_lowercase();
            let mut score = 0i64;
            for t in &terms {
                if path.contains(t) {
                    score += 10;
                    if Path::new(&path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(t))
                        .unwrap_or(false)
                    {
                        score += 10;
                    }
                }
                for s in &f.symbols {
                    if s.to_lowercase().contains(t) {
                        score += 5;
                        break;
                    }
                }
            }
            (score, f)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(limit.max(1))
        .map(|(_, f)| f)
        .collect()
}

/// Compact tree text for prompts (dirs + notable files, capped).
pub fn tree_text(root: &Path, cap_bytes: usize) -> String {
    let idx = build(root);
    let mut dirs: HashMap<String, Vec<String>> = HashMap::new();
    for f in &idx.files {
        let dir = f
            .path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_else(|| ".".into());
        dirs.entry(dir)
            .or_default()
            .push(f.path.rsplit('/').next().unwrap_or(&f.path).to_string());
    }
    let mut dir_names: Vec<_> = dirs.keys().collect();
    dir_names.sort();
    let mut out = String::new();
    for d in dir_names.into_iter().take(80) {
        out.push_str(&format!("{d}/\n"));
        let mut kids = dirs[d].clone();
        kids.sort();
        for k in kids.into_iter().take(15) {
            out.push_str(&format!("  {k}\n"));
        }
    }
    if out.len() > cap_bytes {
        out.truncate(cap_bytes);
        out.push_str("\n…(truncated)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> std::path::PathBuf {
        // Rust runs tests in parallel in one process. A process-ID-only path
        // lets one test erase another's fixture while build() is reading it.
        let dir = std::env::temp_dir().join(format!("companion-idx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap(); // skipped
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\nstruct App {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# demo\n").unwrap();
        std::fs::write(dir.join("target/x"), "junk").unwrap();
        dir
    }

    #[test]
    fn fixtures_are_independent_within_the_same_process() {
        let first = rig();
        let second = rig();
        assert_ne!(first, second);
        std::fs::remove_dir_all(&first).unwrap();
        let idx = build(&second);
        assert!(idx.files.iter().any(|file| file.path == "src/main.rs"));
        std::fs::remove_dir_all(&second).unwrap();
    }

    #[test]
    fn index_skips_build_dirs_and_extracts_symbols() {
        let dir = rig();
        let idx = build(&dir);
        assert!(idx.files.iter().any(|f| f.path == "src/main.rs"));
        assert!(!idx.files.iter().any(|f| f.path.starts_with("target/")));
        let main = idx.files.iter().find(|f| f.path == "src/main.rs").unwrap();
        assert!(main.symbols.iter().any(|s| s.starts_with("fn main")));
        assert!(main.symbols.iter().any(|s| s.starts_with("struct App")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_prefers_filename_hits() {
        let dir = rig();
        let idx = build(&dir);
        let hits = search(&idx, "main", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "src/main.rs");
        assert!(search(&idx, "zzz-nope", 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
