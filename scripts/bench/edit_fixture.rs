        None => (
            old.parse::<usize>()
                .map_err(|_| format!("bad hunk header: {line}"))?,
            1,
        ),
    };
    Ok((start, len))
}

/// Recursive regex search (§24). Large source files are scanned line by line.
/// Returns `path:line: match` lines, capped — deterministic and terse for
/// small local models (§95: relevant sections, not dumps).
fn search_dir(dir: &std::path::Path, re: &regex::Regex) -> String {
    let mut hits: Vec<String> = vec![];
    if dir.is_file() {
        search_file(dir, dir.parent().unwrap_or(dir), re, &mut hits);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if hits.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            if hits.len() >= MAX_SEARCH_MATCHES {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(
                    name.as_str(),
                    "node_modules" | "target" | "build" | "dist" | "vendor" | "__pycache__"
                )
            {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                search_file(&path, dir, re, &mut hits);
            }
        }
    }
    if hits.is_empty() {
        "(no matches)".into()
    } else {
        let mut out = hits.join("\n");
        if hits.len() >= MAX_SEARCH_MATCHES {
            out.push_str(&format!("\n(capped at {MAX_SEARCH_MATCHES} matches)"));
        }
        out
    }
}

fn search_file(
    path: &std::path::Path,
    root: &std::path::Path,
    re: &regex::Regex,
    hits: &mut Vec<String>,
) {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = std::io::BufReader::new(file);
    // Avoid interpreting binary/model assets as code, without a source-size cap.
    match reader.fill_buf() {
        Ok(bytes) if !bytes.contains(&0) => {}
        _ => return,
    }
    for (n, line) in reader.lines().enumerate() {
        if hits.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let Ok(line) = line else {
            break;
        };
        if line.contains('\0') {
            break;
        }
        if re.is_match(&line) {
            hits.push(format!(
                "{}:{}: {}",
                path.strip_prefix(root).unwrap_or(path).display(),
                n + 1,
                line.chars().take(200).collect::<String>()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws() -> WorkspaceManager {
        // Unique per call: parallel tests must never share a scratch dir.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("companion-tool-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        WorkspaceManager::new(dir)
    }

    #[test]
    fn registry_marks_command_dangerous() {
        let reg = registry();
        let cmd = reg.iter().find(|t| t.name == "execute_command").unwrap();
        assert_eq!(cmd.risk, RiskLevel::Dangerous);
    }

    #[test]
    fn search_large_source_and_then_read_deep_match() {
        let w = ws();
        let text = format!(
            "{}deep_unique_marker\n",
            format!("{}\n", "x".repeat(90)).repeat(17_000)
        );
        std::fs::write(w.root().join("large.cpp"), text).unwrap();
        for path in [".", "large.cpp"] {
            let request = ToolRequest {
                name: "search_text".into(),
                args: serde_json::json!({"path":path,"query":"deep_unique_marker"}),
                approved: false,
            };
            let result = execute(&request, &w, false).unwrap();
            assert!(result
