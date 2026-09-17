//! Permission system (§25–§26, §91–§93).
//!
//! Rules:
//! - The LLM is the decision-maker, never the security boundary (§93).
//! - Every tool independently validates paths/args/limits.
//! - Default autonomy is Level 1 (Assisted): everything needs confirmation.

use serde::{Deserialize, Serialize};

/// §25 risk tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    Safe,
    Moderate,
    Dangerous,
}

/// §91 autonomy levels. Default = Assisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutonomyLevel {
    /// Level 0 — Chat: no tools at all.
    Chat = 0,
    /// Level 1 — Assisted: every tool call needs confirmation (default).
    #[default]
    Assisted = 1,
    /// Level 2 — Workspace agent: SAFE tools auto-run inside workspace.
    WorkspaceAgent = 2,
    /// Level 3 — Autonomous: registered tools run without approval prompts
    /// (explicit opt-in). Tool path, argument, mode and search gates still apply.
    Autonomous = 3,
    /// Accept edits: SAFE tools and file edits in the workspace run without
    /// asking; commands, deletion, Git, opening paths and web search still ask
    /// (the Claude Code mode of the same name).
    AcceptEdits = 4,
}

/// The permission modes a code session cycles through (owner decision
/// 2026-09-17, modelled on Claude Code): what each lets run without asking.
/// "plan" also runs the agent read-only; its reads run freely like "ask".
pub const PERMISSION_MODES: [&str; 4] = ["ask", "accept_edits", "plan", "auto"];

/// The autonomy level a permission mode applies. Unknown names read as "ask".
pub fn autonomy_for_mode(mode: &str) -> AutonomyLevel {
    match mode {
        "auto" => AutonomyLevel::Autonomous,
        "accept_edits" => AutonomyLevel::AcceptEdits,
        _ => AutonomyLevel::WorkspaceAgent,
    }
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}

pub struct PermissionManager {
    pub autonomy: AutonomyLevel,
    pub allow_network: bool,
    pub trusted_dirs: Vec<String>,
    /// "Allow for session" grants: (tool, workspace) pairs the user approved.
    grants: std::collections::HashSet<(String, String)>,
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self {
            autonomy: AutonomyLevel::Assisted,
            allow_network: false,
            trusted_dirs: vec![],
            grants: Default::default(),
        }
    }
}

impl PermissionManager {
    pub fn new(autonomy: AutonomyLevel) -> Self {
        Self {
            autonomy,
            ..Self::default()
        }
    }

    /// Human-readable policy summary for /permissions (§91).
    pub fn describe(&self) -> String {
        let mode = match self.autonomy {
            AutonomyLevel::Chat => "Level 0 — Chat (no tools)",
            AutonomyLevel::Assisted => "Level 1 — Assisted (every tool asks first)",
            AutonomyLevel::WorkspaceAgent => {
                "Level 2 — Workspace Agent (safe tools auto-run in workspace)"
            }
            AutonomyLevel::Autonomous => {
                "Level 3 — Auto (registered actions, including commands and deletion, run without per-action approval; tool safety limits remain enforced)"
            }
            AutonomyLevel::AcceptEdits => {
                "Accept edits (reads and file edits in the project run without asking; commands, deletion, Git and web search ask)"
            }
        };
        format!(
            "{mode}\nSession grants: {} (moderate tools, per workspace)\nWeb search: requires the request's Search switch and its configured search policy.",
            self.grants.len(),
        )
    }

    /// Record an "Allow for session" grant (MODERATE tools only).
    pub fn grant_session(&mut self, tool: &str, workspace: &str) {
        self.grants
            .insert((tool.to_string(), workspace.to_string()));
    }

    fn session_granted(&self, tool: &str, workspace: &str) -> bool {
        self.grants
            .contains(&(tool.to_string(), workspace.to_string()))
    }

    /// Central gate: LLM -> ToolRequest -> PermissionManager -> Tool -> OS (§92).
    /// `workspace` is the canonical workspace root when known (for grants).
    pub fn decide(&self, tool: &str, risk: RiskLevel, in_workspace: bool) -> PermissionDecision {
        self.decide_in(tool, risk, in_workspace, None)
    }

    /// `decide_in` for one concrete call, whose arguments can make an
    /// otherwise automatic action ask (an edit inside `.git` in Accept edits).
    pub fn decide_call(
        &self,
        tool: &str,
        args: &serde_json::Value,
        risk: RiskLevel,
        in_workspace: bool,
        workspace: Option<&str>,
    ) -> PermissionDecision {
        if self.autonomy == AutonomyLevel::AcceptEdits && crate::tools::touches_git_internals(tool, args) {
            return PermissionDecision::RequireApproval {
                reason: format!("'{tool}' edits a file inside .git, where git keeps commands it runs; this asks even in Accept edits mode."),
            };
        }
        self.decide_in(tool, risk, in_workspace, workspace)
    }

    pub fn decide_in(
        &self,
        tool: &str,
        risk: RiskLevel,
        in_workspace: bool,
        workspace: Option<&str>,
    ) -> PermissionDecision {
        if self.autonomy == AutonomyLevel::Chat {
            return PermissionDecision::Deny {
                reason: "Tool use disabled in Chat mode (Level 0).".into(),
            };
        }
        if self.autonomy == AutonomyLevel::Autonomous {
            if !in_workspace {
                return PermissionDecision::Deny {
                    reason: "Auto does not grant access outside the selected workspace. Tool boundaries still apply.".into(),
                };
            }
            if !crate::tools::registry()
                .iter()
                .any(|descriptor| descriptor.name == tool)
            {
                return PermissionDecision::Deny {
                    reason: format!("'{tool}' is not a registered tool."),
                };
            }
            // Auto is the user's explicit permission to perform the task's
            // supported actions. It is not a sandbox: command validation,
            // filesystem boundaries, Plan/Ask mode and Search consent are
            // independently enforced by their executors.
            return PermissionDecision::Allow;
        }
        // Session grants auto-allow MODERATE tools in their workspace.
        if risk == RiskLevel::Moderate && in_workspace {
            if let Some(ws) = workspace {
                if self.session_granted(tool, ws) {
                    return PermissionDecision::Allow;
                }
            }
        }
        match (self.autonomy, risk) {
            (AutonomyLevel::Chat, _) => PermissionDecision::Deny {
                reason: "Tool use disabled in Chat mode (Level 0).".into(),
            },
            (_, RiskLevel::Dangerous) => PermissionDecision::RequireApproval {
                reason: format!("'{tool}' needs approval in this mode. Shell commands are not sandboxed by the project folder; deletion may be irreversible."),
            },
            (AutonomyLevel::Assisted, _) => PermissionDecision::RequireApproval {
                reason: format!("Assisted mode: confirm '{tool}' to continue."),
            },
            (AutonomyLevel::AcceptEdits, RiskLevel::Safe) if in_workspace => PermissionDecision::Allow,
            (AutonomyLevel::AcceptEdits, RiskLevel::Moderate) if in_workspace && crate::tools::is_file_edit(tool) => {
                PermissionDecision::Allow
            }
            (AutonomyLevel::AcceptEdits, _) => PermissionDecision::RequireApproval {
                reason: format!("'{tool}' needs approval in Accept edits mode: only reads and file edits run without asking."),
            },
            (AutonomyLevel::WorkspaceAgent, RiskLevel::Safe) if in_workspace => {
                PermissionDecision::Allow
            }
            (AutonomyLevel::WorkspaceAgent, _) => PermissionDecision::RequireApproval {
                reason: format!("'{tool}' needs confirmation outside auto-approved SAFE set."),
            },
            (AutonomyLevel::Autonomous, _) => PermissionDecision::RequireApproval {
                reason: format!("'{tool}' could not be validated for Auto mode."),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_assisted_and_gates_safe_tools() {
        let pm = PermissionManager::default();
        assert_eq!(pm.autonomy, AutonomyLevel::Assisted);
        assert!(matches!(
            pm.decide("read_file", RiskLevel::Safe, true),
            PermissionDecision::RequireApproval { .. }
        ));
    }

    #[test]
    fn autonomous_runs_registered_actions_without_per_tool_prompts() {
        let pm = PermissionManager::new(AutonomyLevel::Autonomous);
        for descriptor in crate::tools::registry() {
            assert!(
                matches!(
                    pm.decide(descriptor.name, descriptor.risk, true),
                    PermissionDecision::Allow
                ),
                "{} unexpectedly asked in Auto",
                descriptor.name
            );
        }
        assert!(matches!(
            pm.decide("execute_command", RiskLevel::Dangerous, false),
            PermissionDecision::Deny { .. }
        ));
        assert!(matches!(
            pm.decide("invented_tool", RiskLevel::Safe, true),
            PermissionDecision::Deny { .. }
        ));
        assert!(pm.describe().contains("without per-action approval"));
    }

    #[test]
    fn assisted_still_asks_for_every_registered_action_without_a_session_grant() {
        let pm = PermissionManager::default();
        for descriptor in crate::tools::registry() {
            assert!(
                matches!(
                    pm.decide(descriptor.name, descriptor.risk, true),
                    PermissionDecision::RequireApproval { .. }
                ),
                "{} bypassed Ask",
                descriptor.name
            );
        }
    }

    #[test]
    fn auto_permission_does_not_remove_the_filesystem_boundary() {
        let pm = PermissionManager::new(AutonomyLevel::Autonomous);
        assert!(matches!(
            pm.decide("write_file", RiskLevel::Moderate, true),
            PermissionDecision::Allow
        ));
        let ws = crate::workspace::WorkspaceManager::new(
            std::env::temp_dir().join("auto-boundary-fixture"),
        );
        assert!(ws.resolve("../outside.txt").is_err());
    }

    #[test]
    fn accept_edits_runs_reads_and_file_edits_and_asks_for_the_rest() {
        let pm = PermissionManager::new(autonomy_for_mode("accept_edits"));
        for tool in ["read_file", "list_directory", "search_text"] {
            assert!(matches!(pm.decide(tool, crate::tools::risk_of(tool), true), PermissionDecision::Allow), "{tool}");
        }
        for tool in ["write_file", "append_file", "edit_file", "create_document"] {
            assert!(matches!(pm.decide(tool, crate::tools::risk_of(tool), true), PermissionDecision::Allow), "{tool}");
            assert!(matches!(pm.decide(tool, crate::tools::risk_of(tool), false), PermissionDecision::RequireApproval { .. }), "{tool} outside");
        }
        for tool in ["execute_command", "delete_file", "git_commit", "open_path", "web_search"] {
            assert!(matches!(pm.decide(tool, crate::tools::risk_of(tool), true), PermissionDecision::RequireApproval { .. }), "{tool}");
        }
    }

    #[test]
    fn accept_edits_asks_for_edits_inside_git_folders() {
        let pm = PermissionManager::new(autonomy_for_mode("accept_edits"));
        let config = serde_json::json!({"path": ".git/config"});
        assert!(matches!(pm.decide_call("write_file", &config, RiskLevel::Moderate, true, None), PermissionDecision::RequireApproval { .. }));
        let page = serde_json::json!({"path": "index.html"});
        assert!(matches!(pm.decide_call("write_file", &page, RiskLevel::Moderate, true, None), PermissionDecision::Allow));
        let auto = PermissionManager::new(autonomy_for_mode("auto"));
        assert!(matches!(auto.decide_call("write_file", &config, RiskLevel::Moderate, true, None), PermissionDecision::Allow), "Auto is Auto");
    }

    #[test]
    fn ask_mode_reads_freely_and_asks_before_edits_and_commands() {
        let pm = PermissionManager::new(autonomy_for_mode("ask"));
        assert!(matches!(pm.decide("read_file", RiskLevel::Safe, true), PermissionDecision::Allow));
        for tool in ["write_file", "edit_file", "execute_command", "delete_file"] {
            assert!(matches!(pm.decide(tool, crate::tools::risk_of(tool), true), PermissionDecision::RequireApproval { .. }), "{tool}");
        }
        assert_eq!(autonomy_for_mode("plan"), AutonomyLevel::WorkspaceAgent);
        assert_eq!(autonomy_for_mode("auto"), AutonomyLevel::Autonomous);
        assert_eq!(autonomy_for_mode("anything else"), AutonomyLevel::WorkspaceAgent);
    }

    #[test]
    fn chat_mode_denies_everything() {
        let pm = PermissionManager::new(AutonomyLevel::Chat);
        assert!(matches!(
            pm.decide("read_file", RiskLevel::Safe, true),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn workspace_agent_auto_allows_safe_in_workspace() {
        let pm = PermissionManager::new(AutonomyLevel::WorkspaceAgent);
        assert!(matches!(
            pm.decide("read_file", RiskLevel::Safe, true),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            pm.decide("read_file", RiskLevel::Safe, false),
            PermissionDecision::RequireApproval { .. }
        ));
    }

    #[test]
    fn session_grant_allows_moderate_but_never_dangerous() {
        let mut pm = PermissionManager::default();
        pm.grant_session("write_file", "C:/ws");
        pm.grant_session("execute_command", "C:/ws");
        assert!(matches!(
            pm.decide_in("write_file", RiskLevel::Moderate, true, Some("C:/ws")),
            PermissionDecision::Allow
        ));
        // Other workspace: still gated.
        assert!(matches!(
            pm.decide_in("write_file", RiskLevel::Moderate, true, Some("C:/other")),
            PermissionDecision::RequireApproval { .. }
        ));
        // A session grant does not silently turn Ask mode into Auto mode.
        assert!(matches!(
            pm.decide_in("execute_command", RiskLevel::Dangerous, true, Some("C:/ws")),
            PermissionDecision::RequireApproval { .. }
        ));
    }
}
