//! Slash command system (§§130–143): deterministic shortcuts, not prompts.
//!
//! Pipeline: input → parser → registry → handler → outcome. Commands never
//! bypass the permission system (§138): reads run directly, writes/builds
//! spawn supervised agent runs, and only implemented commands are listed.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandRisk {
    Safe,
    Moderate,
    High,
}

#[derive(Debug, Clone)]
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub category: &'static str,
    pub risk: CommandRisk,
    pub usage: &'static str,
}

pub fn registry() -> Vec<CommandDef> {
    vec![
        CommandDef {
            name: "help",
            aliases: &[],
            description: "Show commands or help for one",
            category: "help",
            risk: CommandRisk::Safe,
            usage: "/help [command]",
        },
        CommandDef {
            name: "clear",
            aliases: &["c"],
            description: "Start a fresh conversation (history kept)",
            category: "conversation",
            risk: CommandRisk::Safe,
            usage: "/clear",
        },
        CommandDef {
            name: "compact",
            aliases: &[],
            description: "Summarize older turns to free context",
            category: "conversation",
            risk: CommandRisk::Safe,
            usage: "/compact",
        },
        CommandDef {
            name: "model",
            aliases: &["m"],
            description: "Show or switch the active model",
            category: "model",
            risk: CommandRisk::Safe,
            usage: "/model [id]",
        },
        CommandDef {
            name: "models",
            aliases: &[],
            description: "List installed models",
            category: "model",
            risk: CommandRisk::Safe,
            usage: "/models",
        },
        CommandDef {
            name: "status",
            aliases: &[],
            description: "Show model, context, hardware, agent state",
            category: "system",
            risk: CommandRisk::Safe,
            usage: "/status",
        },
        CommandDef {
            name: "context",
            aliases: &[],
            description: "Show context usage for this chat",
            category: "context",
            risk: CommandRisk::Safe,
            usage: "/context",
        },
        CommandDef {
            name: "search",
            aliases: &["s"],
            description: "Search the linked workspace",
            category: "coding",
            risk: CommandRisk::Safe,
            usage: "/search <query>",
        },
        CommandDef {
            name: "find",
            aliases: &["ls"],
            description: "Find files by name",
            category: "coding",
            risk: CommandRisk::Safe,
            usage: "/find <pattern>",
        },
        CommandDef {
            name: "diff",
            aliases: &["d"],
            description: "Show workspace git changes",
            category: "coding",
            risk: CommandRisk::Safe,
            usage: "/diff",
        },
        CommandDef {
            name: "review",
            aliases: &[],
            description: "Agent reviews code without modifying",
            category: "coding",
            risk: CommandRisk::Safe,
            usage: "/review [path]",
        },
        CommandDef {
            name: "plan",
            aliases: &[],
            description: "Agent makes a plan without changing files",
            category: "agent",
            risk: CommandRisk::Safe,
            usage: "/plan <task>",
        },
        CommandDef {
            name: "init",
            aliases: &[],
            description: "Inspect repo and summarize the project",
            category: "coding",
            risk: CommandRisk::Safe,
            usage: "/init",
        },
        CommandDef {
            name: "build",
            aliases: &[],
            description: "Agent builds the project (asks first)",
            category: "coding",
            risk: CommandRisk::Moderate,
            usage: "/build [target]",
        },
        CommandDef {
            name: "test",
            aliases: &[],
            description: "Agent runs project tests (asks first)",
            category: "coding",
            risk: CommandRisk::Moderate,
            usage: "/test [filter]",
        },
        CommandDef {
            name: "run",
            aliases: &[],
            description: "Agent runs a project command (asks first)",
            category: "coding",
            risk: CommandRisk::High,
            usage: "/run <command...>",
        },
        CommandDef {
            name: "permissions",
            aliases: &[],
            description: "Show agent permission policy",
            category: "system",
            risk: CommandRisk::High,
            usage: "/permissions",
        },
        CommandDef {
            name: "config",
            aliases: &[],
            description: "Show app/agent/search configuration",
            category: "system",
            risk: CommandRisk::Safe,
            usage: "/config [section]",
        },
        CommandDef {
            name: "stop",
            aliases: &[],
            description: "Stop the active generation/agent",
            category: "agent",
            risk: CommandRisk::Safe,
            usage: "/stop",
        },
        CommandDef {
            name: "retry",
            aliases: &[],
            description: "Retry the last user turn",
            category: "agent",
            risk: CommandRisk::Safe,
            usage: "/retry",
        },
    ]
}

pub fn find(name: &str) -> Option<CommandDef> {
    let name = name.strip_prefix('/').unwrap_or(name);
    registry()
        .into_iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

/// Split "/name arg1 arg2" → ("name", "arg1 arg2"). None when not a command.
pub fn parse(input: &str) -> Option<(String, String)> {
    let t = input.trim();
    if !t.starts_with('/') {
        return None;
    }
    // A lone "/" opens autocomplete; "//" is literal text.
    if t == "/" || t.starts_with("//") {
        return None;
    }
    let body = t[1..].trim();
    if body.is_empty() {
        return None;
    }
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").to_string();
    let args = parts.next().unwrap_or("").trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some((name, args))
}

/// Prefix completion for the composer menu (§132).
pub fn complete(prefix: &str) -> Vec<CommandDef> {
    let p = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    registry()
        .into_iter()
        .filter(|c| c.name.starts_with(&p) || c.aliases.iter().any(|a| a.starts_with(&p)))
        .collect()
}

pub fn help_text(name: Option<&str>) -> String {
    if let Some(n) = name {
        if let Some(c) = find(n) {
            return format!(
                "/{name}\n{desc}\nUsage: {usage}\nCategory: {cat} · Risk: {risk:?}\nAliases: {aliases}",
                name = c.name, desc = c.description, usage = c.usage, cat = c.category, risk = c.risk,
                aliases = if c.aliases.is_empty() { "—".into() } else { c.aliases.join(", ") },
            );
        }
        return format!("Unknown command '/{n}'. Try /help.");
    }
    let mut out = String::from("Commands (deterministic shortcuts — no interpretation):\n");
    out.push_str("/clear starts a FRESH context (env preserved); /compact CONDENSES this one (continuity preserved).\n");
    let mut cat = "";
    let mut all = registry();
    all.sort_by_key(|c| (c.category, c.name));
    for c in all {
        if c.category != cat {
            cat = c.category;
            out.push_str(&format!("\n{cat}:\n"));
        }
        out.push_str(&format!("  /{:<10} {}\n", c.name, c.description));
    }
    out
}

/// Filename search for /find: substring or `*.ext` / `prefix*` globs.
/// Skips hidden dirs, caps output for small models.
pub fn find_files(root: &std::path::Path, pattern: &str, cap: usize) -> Vec<String> {
    let pat = pattern.trim().to_lowercase();
    let (mode, needle) = if pat.starts_with("*.") {
        ("ext", pat[2..].to_string())
    } else if pat.ends_with('*') {
        ("prefix", pat[..pat.len() - 1].to_string())
    } else {
        ("sub", pat)
    };
    let mut hits = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if hits.len() >= cap {
            break;
        }
        let mut entries: Vec<_> = std::fs::read_dir(&d)
            .map(|e| e.flatten().collect())
            .unwrap_or_default();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            if hits.len() >= cap {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            let lower = name.to_lowercase();
            let matched = match mode {
                "ext" => lower.ends_with(&format!(".{needle}")),
                "prefix" => lower.starts_with(&needle),
                _ => lower.contains(&needle),
            };
            if matched {
                hits.push(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_args() {
        assert_eq!(
            parse("/search temporal").unwrap(),
            ("search".into(), "temporal".into())
        );
        assert_eq!(parse("/help").unwrap(), ("help".into(), "".into()));
        assert_eq!(
            parse("/model qwen-14b").unwrap(),
            ("model".into(), "qwen-14b".into())
        );
        assert!(parse("not a command").is_none());
        assert!(parse("/").is_none());
        assert!(parse("// not a command").is_none());
        assert!(parse("/ commanding with space").is_some());
    }

    #[test]
    fn aliases_and_prefix_completion() {
        assert_eq!(find("s").unwrap().name, "search");
        assert_eq!(find("d").unwrap().name, "diff");
        assert!(find("nope").is_none());
        let names: Vec<_> = complete("/re").iter().map(|c| c.name).collect();
        assert!(names.contains(&"review"));
        assert!(names.contains(&"retry"));
        assert!(!names.contains(&"search"));
    }

    #[test]
    fn help_lists_only_implemented() {
        let h = help_text(None);
        assert!(h.contains("/build"));
        assert!(
            !h.contains("/doctor"),
            "unimplemented commands must not be advertised"
        );
        assert!(help_text(Some("build")).contains("Usage:"));
        assert!(help_text(Some("nope")).contains("Unknown command"));
    }

    #[test]
    fn find_files_matches_patterns() {
        let root = std::env::temp_dir().join(format!("companion-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("temporal.cpp"), "").unwrap();
        std::fs::write(root.join("src").join("main.rs"), "").unwrap();
        let hits = find_files(&root, "*.cpp", 10);
        assert_eq!(hits.len(), 1);
        assert!(find_files(&root, "temporal", 10).len() == 1);
        assert!(find_files(&root, "zzz-nope", 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
