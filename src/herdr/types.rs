//! The subset of Herdr's JSON API that the inbox reads.
//!
//! Every type tolerates fields it does not know and every enum has a fallback,
//! because Herdr adds fields and values between releases.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    /// Waiting for the user: a permission prompt, a question, a dialog.
    Blocked,
    /// Idle after finishing work the user has not looked at yet.
    Done,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub terminal_id: String,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub state_change_seq: u64,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeInfo {
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub worktree: Option<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
}

/// `ping`: who is answering, and what it can do.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pong {
    pub version: String,
}

/// A change to one agent pane, from a `pane.agent_status_changed` subscription.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentStatusChange {
    pub pane_id: String,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_values_parse_and_new_values_fall_back_to_unknown() {
        let parse = |value: &str| serde_json::from_value::<AgentStatus>(json!(value)).unwrap();
        assert_eq!(parse("idle"), AgentStatus::Idle);
        assert_eq!(parse("working"), AgentStatus::Working);
        assert_eq!(parse("blocked"), AgentStatus::Blocked);
        assert_eq!(parse("done"), AgentStatus::Done);
        assert_eq!(parse("unknown"), AgentStatus::Unknown);
        assert_eq!(parse("thinking-hard"), AgentStatus::Unknown);
    }

    #[test]
    fn agent_info_parses_a_real_shape_and_ignores_unknown_fields() {
        let agent: AgentInfo = serde_json::from_value(json!({
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id", "source": "herdr:claude", "value": "x"},
            "agent_status": "idle",
            "cwd": "/w/repo",
            "focused": false,
            "foreground_cwd": "/w/repo",
            "pane_id": "w1S:p1",
            "revision": 2,
            "state_change_seq": 7,
            "tab_id": "w1S:t1",
            "terminal_id": "term_1",
            "terminal_title": "✳ Sidebar improvements",
            "terminal_title_stripped": "Sidebar improvements",
            "workspace_id": "w1S",
            "some_future_field": {"nested": true}
        }))
        .unwrap();
        assert_eq!(agent.pane_id, "w1S:p1");
        assert_eq!(agent.agent.as_deref(), Some("claude"));
        assert_eq!(agent.agent_status, AgentStatus::Idle);
        assert_eq!(agent.state_change_seq, 7);
        assert_eq!(agent.terminal_title_stripped.as_deref(), Some("Sidebar improvements"));
        assert!(agent.tokens.is_empty());
    }

    #[test]
    fn agent_info_requires_its_identity_fields() {
        let missing_pane = json!({"workspace_id": "w1", "tab_id": "w1:t1"});
        assert!(serde_json::from_value::<AgentInfo>(missing_pane).is_err());
    }

    #[test]
    fn snapshot_tolerates_missing_collections() {
        let snapshot: SessionSnapshot = serde_json::from_value(json!({"version": "0.9.3"})).unwrap();
        assert!(snapshot.agents.is_empty());
        assert!(snapshot.workspaces.is_empty());
    }

    #[test]
    fn workspace_worktree_is_optional() {
        let plain: WorkspaceInfo =
            serde_json::from_value(json!({"workspace_id": "w1", "label": "lucas"})).unwrap();
        assert_eq!(plain.worktree, None);
        let linked: WorkspaceInfo = serde_json::from_value(json!({
            "workspace_id": "w2",
            "label": "fix",
            "worktree": {
                "repo_key": "k", "repo_name": "cockpit", "repo_root": "/w/cockpit",
                "checkout_path": "/h/.herdr/worktrees/cockpit/fix", "is_linked_worktree": true
            }
        }))
        .unwrap();
        let worktree = linked.worktree.unwrap();
        assert_eq!(worktree.repo_name, "cockpit");
        assert!(worktree.is_linked_worktree);
    }
}
