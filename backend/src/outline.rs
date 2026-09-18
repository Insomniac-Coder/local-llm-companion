//! What is in a file, or in a folder, without reading it all (owner request,
//! 2026-09-18).
//!
//! Reading is how a model explores, and reading is what fills the window: one
//! run read fourteen whole files to find where things were, another paged
//! through the same 450-line component in slices over and over (night decision
//! 61). An outline answers "what is in here and on which line" for a fraction
//! of the text, and the line numbers feed `replace_lines` directly.

use std::path::Path;

/// Symbols listed for one file before the rest are summarized away.
const MAX_SYMBOLS: usize = 200;
/// Entries listed for one folder.
const MAX_ENTRIES: usize = 120;
const MAX_SIGNATURE: usize = 110;
/// A file this large is not read for symbols; its size is reported instead.
const MAX_SCAN_BYTES: u64 = 2_000_000;

fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".next" | ".venv" | "venv"
            | "__pycache__" | ".cache" | "vendor" | "out" | "bin" | "obj"
    )
}

/// Line starts that name something worth finding again, per language. Kept
/// deliberately plain: this is a map, not a parser, and a wrong guess costs a
/// line of noise rather than a wrong answer.
fn markers(ext: &str) -> &'static [&'static str] {
    match ext {
        "rs" => &["fn ", "pub fn ", "struct ", "pub struct ", "enum ", "pub enum ", "trait ", "pub trait ", "impl ", "mod ", "pub mod ", "macro_rules!", "const ", "pub const ", "static ", "type "],
        "py" => &["def ", "async def ", "class "],
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => &[
            "function ", "async function ", "class ", "const ", "let ", "var ", "type ", "interface ", "enum ",
            "export ", "export default ", "module.exports",
        ],
        "go" => &["func ", "type ", "const ", "var "],
        "java" | "kt" | "cs" => &["class ", "interface ", "enum ", "record ", "public ", "private ", "protected ", "internal ", "fun ", "object "],
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => &["class ", "struct ", "namespace ", "template", "void ", "int ", "bool ", "float ", "double ", "auto ", "static ", "#define "],
        "rb" => &["def ", "class ", "module "],
        "php" => &["function ", "class ", "trait ", "interface "],
        "swift" => &["func ", "class ", "struct ", "enum ", "protocol ", "extension "],
        "css" | "scss" | "less" => &[".", "#", "@media", "@keyframes", ":root"],
        "md" => &["# ", "## ", "### "],
        "html" => &["<section", "<main", "<header", "<footer", "<nav", "<form", "<script", "<style"],
        _ => &[],
    }
}

fn is_comment(line: &str) -> bool {
    line.starts_with("//") || line.starts_with('#') && !line.starts_with("#define") || line.starts_with("*")
        || line.starts_with("/*") || line.starts_with("--")
}

/// The symbols of one file, as `line: signature`.
pub fn file_outline(path: &Path, display: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {display}: {e}"))?;
    if meta.len() > MAX_SCAN_BYTES {
        return Ok(format!(
            "{display}: {} bytes, too large to outline. Read the part you need with read_file.",
            meta.len()
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {display}: {e}"))?;
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    let markers = markers(&ext);
    let lines: Vec<&str> = text.lines().collect();
    let mut found: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    if !markers.is_empty() {
        for (index, raw) in lines.iter().enumerate() {
            let line = raw.trim();
            // Markdown headings and CSS selectors are the line; code is not.
            if line.is_empty() || (ext != "md" && ext != "css" && ext != "scss" && ext != "less" && is_comment(line)) {
                continue;
            }
            if !markers.iter().any(|marker| line.starts_with(marker)) {
                continue;
            }
            if ext == "css" || ext == "scss" || ext == "less" {
                // A selector ends in a brace; a property does not.
                if !line.ends_with('{') {
                    continue;
                }
            }
            if found.len() >= MAX_SYMBOLS {
                skipped += 1;
                continue;
            }
            let signature: String = line.chars().take(MAX_SIGNATURE).collect();
            found.push(format!("{}: {signature}", index + 1));
        }
    }
    let mut out = format!("{display}: {} lines", lines.len());
    if markers.is_empty() {
        out.push_str("\n(no outline for this file type; read_file shows its text)");
        return Ok(out);
    }
    if found.is_empty() {
        out.push_str("\n(nothing that looks like a definition; read_file shows its text)");
        return Ok(out);
    }
    out.push_str(&format!(", {} definitions:\n", found.len() + skipped));
    out.push_str(&found.join("\n"));
    if skipped > 0 {
        out.push_str(&format!("\n[{skipped} more not listed]"));
    }
    Ok(out)
}

/// What a folder holds: its files with their sizes and line counts, and its
/// subfolders with how much is inside them.
pub fn folder_outline(path: &Path, display: &str) -> Result<String, String> {
    let mut files: Vec<(String, u64, Option<usize>)> = Vec::new();
    let mut folders: Vec<(String, usize)> = Vec::new();
    let entries = std::fs::read_dir(path).map_err(|e| format!("cannot list {display}: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if skip_dir(&name) {
                folders.push((name, 0));
                continue;
            }
            let inside = std::fs::read_dir(entry.path())
                .map(|read| read.flatten().count())
                .unwrap_or(0);
            folders.push((name, inside));
        } else {
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            let lines = (size < MAX_SCAN_BYTES)
                .then(|| std::fs::read_to_string(entry.path()).ok().map(|text| text.lines().count()))
                .flatten();
            files.push((name, size, lines));
        }
    }
    folders.sort();
    files.sort();
    let mut out = format!(
        "{display}: {} file(s), {} folder(s)",
        files.len(),
        folders.len()
    );
    let mut listed = 0usize;
    for (name, inside) in &folders {
        if listed >= MAX_ENTRIES {
            break;
        }
        listed += 1;
        out.push_str(&format!(
            "\n{name}/ ({})",
            if skip_dir(name) {
                "not searched".to_string()
            } else {
                format!("{inside} entries")
            }
        ));
    }
    for (name, size, lines) in &files {
        if listed >= MAX_ENTRIES {
            break;
        }
        listed += 1;
        match lines {
            Some(count) => out.push_str(&format!("\n{name} ({count} lines, {size} bytes)")),
            None => out.push_str(&format!("\n{name} ({size} bytes)")),
        }
    }
    let total = files.len() + folders.len();
    if total > listed {
        out.push_str(&format!("\n[{} more not listed]", total - listed));
    }
    Ok(out)
}

pub fn outline(path: &Path, display: &str) -> Result<String, String> {
    if path.is_dir() {
        folder_outline(path, display)
    } else {
        file_outline(path, display)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("companion-outline-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_is_mapped_to_its_definitions_with_the_lines_to_edit() {
        let dir = temp("file");
        let path = dir.join("app.ts");
        std::fs::write(
            &path,
            "import React from 'react';\n\
             // a comment that names function nothing\n\
             \n\
             export interface Planet {\n  name: string;\n}\n\
             \n\
             export function orbit(planet: Planet) {\n  return planet;\n}\n\
             \n\
             const SPEEDS = [1, 10, 100];\n",
        )
        .unwrap();
        let out = outline(&path, "src/app.ts").unwrap();
        assert!(out.starts_with("src/app.ts: 12 lines, 3 definitions:"), "{out}");
        assert!(out.contains("4: export interface Planet {"), "{out}");
        assert!(out.contains("8: export function orbit(planet: Planet) {"), "{out}");
        assert!(out.contains("12: const SPEEDS = [1, 10, 100];"), "{out}");
        assert!(!out.contains("a comment that names"), "comments are not definitions: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_mapped_to_its_files_and_what_is_worth_opening() {
        let dir = temp("folder");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/react")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# title\nbody\n").unwrap();
        let out = outline(&dir, ".").unwrap();
        assert!(out.starts_with(".: 1 file(s), 2 folder(s)"), "{out}");
        assert!(out.contains("src/ (1 entries)"), "{out}");
        assert!(out.contains("node_modules/ (not searched)"), "{out}");
        assert!(out.contains("README.md (2 lines, "), "{out}");
        // A file type with no map says so rather than pretending it is empty.
        std::fs::write(dir.join("data.bin"), [0u8, 1, 2]).unwrap();
        let binary = outline(&dir.join("data.bin"), "data.bin").unwrap();
        assert!(binary.contains("no outline for this file type"), "{binary}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
