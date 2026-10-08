//! Threads: one per agent pane, labelled and ordered for the inbox.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use crate::git::Checkout;
use crate::herdr::types::{AgentInfo, AgentStatus, SessionSnapshot, TabInfo, WorkspaceInfo};

/// Stable identity of a thread across refreshes.
pub type ThreadId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    NeedsInput,
    Ready,
    Working,
    Idle,
    Unknown,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::NeedsInput, Group::Ready, Group::Working, Group::Idle, Group::Unknown];

    pub fn of(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Blocked => Group::NeedsInput,
            AgentStatus::Done => Group::Ready,
            AgentStatus::Working => Group::Working,
            AgentStatus::Idle => Group::Idle,
            AgentStatus::Unknown => Group::Unknown,
        }
    }

    pub fn heading(self) -> &'static str {
        match self {
            Group::NeedsInput => "needs input",
            Group::Ready => "ready",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: ThreadId,
    pub pane_id: String,
    pub workspace_id: String,
    pub status: AgentStatus,
    pub title: String,
    pub project: String,
    pub branch: Option<String>,
    pub harness: String,
    /// When the status last changed, if this client saw it change.
    pub changed_at: Option<SystemTime>,
    /// Herdr's per-server change counter, used to order threads whose change
    /// time is unknown.
    pub change_seq: u64,
}

impl Thread {
    pub fn group(&self) -> Group {
        Group::of(self.status)
    }
}

/// What this client knows that Herdr does not: when statuses changed, and
/// which finished threads the user already opened here.
#[derive(Debug, Default, Clone)]
pub struct Activity {
    entries: HashMap<ThreadId, Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    status: AgentStatus,
    changed_at: Option<SystemTime>,
    seen_done: bool,
}

impl Activity {
    /// Records a status observation. The first observation of a thread has no
    /// change time: the inbox did not see it change.
    pub fn observe(&mut self, id: &str, status: AgentStatus, now: SystemTime) {
        match self.entries.get_mut(id) {
            Some(entry) if entry.status == status => {}
            Some(entry) => {
                entry.status = status;
                entry.changed_at = Some(now);
                entry.seen_done = false;
            }
            None => {
                self.entries.insert(
                    id.to_string(),
                    Entry {
                        status,
                        changed_at: None,
                        seen_done: false,
                    },
                );
            }
        }
    }

    /// The user opened this thread: a finished thread stops asking for attention.
    pub fn mark_seen(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            if entry.status == AgentStatus::Done {
                entry.seen_done = true;
            }
        }
    }

    pub fn changed_at(&self, id: &str) -> Option<SystemTime> {
        self.entries.get(id).and_then(|entry| entry.changed_at)
    }

    /// Herdr's status, with finished threads the user opened shown as idle.
    pub fn effective(&self, id: &str, status: AgentStatus) -> AgentStatus {
        match self.entries.get(id) {
            Some(entry) if status == AgentStatus::Done && entry.status == AgentStatus::Done && entry.seen_done => {
                AgentStatus::Idle
            }
            _ => status,
        }
    }

    pub fn retain(&mut self, live: &HashSet<ThreadId>) {
        self.entries.retain(|id, _| live.contains(id));
    }
}

/// Builds the inbox's threads from a snapshot, sorted for display.
pub fn build(
    snapshot: &SessionSnapshot,
    activity: &Activity,
    checkout: &dyn Fn(&Path) -> Option<Checkout>,
) -> Vec<Thread> {
    let workspaces: HashMap<&str, &WorkspaceInfo> =
        snapshot.workspaces.iter().map(|w| (w.workspace_id.as_str(), w)).collect();
    let tabs: HashMap<&str, &TabInfo> = snapshot.tabs.iter().map(|t| (t.tab_id.as_str(), t)).collect();
    let mut threads: Vec<Thread> = snapshot
        .agents
        .iter()
        .map(|agent| {
            let workspace = workspaces.get(agent.workspace_id.as_str()).copied();
            let tab = tabs.get(agent.tab_id.as_str()).copied();
            thread(agent, workspace, tab, activity, checkout)
        })
        .collect();
    sort(&mut threads);
    threads
}

fn thread(
    agent: &AgentInfo,
    workspace: Option<&WorkspaceInfo>,
    tab: Option<&TabInfo>,
    activity: &Activity,
    checkout: &dyn Fn(&Path) -> Option<Checkout>,
) -> Thread {
    let id = agent.pane_id.clone();
    let path = workspace
        .and_then(|w| w.worktree.as_ref())
        .map(|w| w.checkout_path.as_str())
        .or(agent.foreground_cwd.as_deref())
        .or(agent.cwd.as_deref());
    let found = path.and_then(|path| checkout(Path::new(path)));
    let project = workspace
        .and_then(|w| w.worktree.as_ref())
        .map(|w| w.repo_name.clone())
        .or_else(|| found.as_ref().map(|c| c.repo.clone()))
        .or_else(|| path.and_then(basename))
        .or_else(|| workspace.map(|w| w.label.clone()).filter(|l| !l.is_empty()))
        .unwrap_or_else(|| "no project".to_string());
    let harness = harness_label(agent);
    Thread {
        title: title(agent, tab, &harness),
        status: activity.effective(&id, agent.agent_status),
        changed_at: activity.changed_at(&id),
        branch: found.and_then(|c| c.branch),
        pane_id: agent.pane_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        change_seq: agent.state_change_seq,
        project,
        harness,
        id,
    }
}

fn basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

/// The best human title Herdr knows for a thread.
pub fn title(agent: &AgentInfo, tab: Option<&TabInfo>, harness: &str) -> String {
    let candidates = [
        agent.tokens.get("thread").map(String::as_str),
        agent.terminal_title_stripped.as_deref(),
        tab.map(|t| t.label.as_str()).filter(|label| !label.chars().all(|c| c.is_ascii_digit())),
        agent.title.as_deref(),
        agent.name.as_deref(),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(clean_title)
        .find(|title| is_meaningful(title, agent))
        .unwrap_or_else(|| format!("New {harness} thread"))
}

/// Strips a leading `[3] ` counter and surrounding whitespace.
fn clean_title(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('[') {
        if let Some((count, tail)) = rest.split_once(']') {
            if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) {
                return tail.trim().to_string();
            }
        }
    }
    raw.to_string()
}

/// Rejects empty titles, shell prompts like `user@host:~/x` and bare agent names.
fn is_meaningful(title: &str, agent: &AgentInfo) -> bool {
    if title.is_empty() {
        return false;
    }
    if let Some((user_host, _)) = title.split_once(':') {
        if user_host.contains('@') && !user_host.contains(' ') {
            return false;
        }
    }
    let lower = title.to_lowercase();
    let names = [agent.agent.as_deref(), agent.display_agent.as_deref()];
    !names.into_iter().flatten().any(|name| name.to_lowercase() == lower)
}

/// "Claude", "OpenCode": Herdr's display name, or a known spelling of its kind.
pub fn harness_label(agent: &AgentInfo) -> String {
    if let Some(display) = agent.display_agent.as_deref().filter(|d| !d.trim().is_empty()) {
        return display.trim().to_string();
    }
    let Some(kind) = agent.agent.as_deref().filter(|k| !k.trim().is_empty()) else {
        return "Agent".to_string();
    };
    match kind {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "opencode" => "OpenCode".into(),
        "pi" => "Pi".into(),
        "gemini" => "Gemini".into(),
        "copilot" => "Copilot".into(),
        "amp" => "Amp".into(),
        "cursor" => "Cursor".into(),
        "grok" => "Grok".into(),
        "kimi" => "Kimi".into(),
        "kiro" => "Kiro".into(),
        "droid" => "Droid".into(),
        "hermes" => "Hermes".into(),
        "kilo" => "Kilo".into(),
        "qwen" => "Qwen".into(),
        "cline" => "Cline".into(),
        "devin" => "Devin".into(),
        "agy" | "antigravity" => "Antigravity".into(),
        "omp" => "OMP".into(),
        "mastracode" => "Mastra Code".into(),
        "qodercli" => "Qoder".into(),
        "letta" => "Letta".into(),
        "maki" => "Maki".into(),
        "muse" => "Muse".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Agent".into(),
            }
        }
    }
}

/// Needs input first, then ready, working, idle, unknown. Inside a group the
/// most recent change comes first; threads without a known change time follow,
/// newest Herdr change counter first. Ties fall back to stable text order so
/// the list never shuffles on refresh.
pub fn sort(threads: &mut [Thread]) {
    threads.sort_by(|a, b| {
        a.group()
            .cmp(&b.group())
            .then_with(|| match (a.changed_at, b.changed_at) {
                (Some(a), Some(b)) => b.cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| b.change_seq.cmp(&a.change_seq))
            .then_with(|| a.project.cmp(&b.project))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::types::WorktreeInfo;
    use std::time::Duration;

    fn agent(pane: &str, status: AgentStatus) -> AgentInfo {
        AgentInfo {
            pane_id: pane.into(),
            workspace_id: pane.split(':').next().unwrap().into(),
            tab_id: format!("{}:t1", pane.split(':').next().unwrap()),
            terminal_id: String::new(),
            agent_status: status,
            agent: Some("claude".into()),
            display_agent: None,
            name: None,
            title: None,
            terminal_title_stripped: None,
            cwd: None,
            foreground_cwd: None,
            tokens: Default::default(),
            state_change_seq: 0,
            focused: false,
        }
    }

    fn no_checkout(_: &Path) -> Option<Checkout> {
        None
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn groups_follow_attention_order() {
        let order: Vec<Group> = [AgentStatus::Unknown, AgentStatus::Idle, AgentStatus::Working, AgentStatus::Done, AgentStatus::Blocked]
            .into_iter()
            .map(Group::of)
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, Group::ALL.to_vec());
    }

    #[test]
    fn the_thread_token_beats_every_other_title() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        a.tokens.insert("thread".into(), "Fix login".into());
        a.terminal_title_stripped = Some("Terminal title".into());
        a.title = Some("Agent title".into());
        assert_eq!(title(&a, None, "Claude"), "Fix login");
    }

    #[test]
    fn title_falls_back_through_terminal_tab_title_name() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        let tab = TabInfo { tab_id: "w1:t1".into(), workspace_id: "w1".into(), label: "Review PR".into() };
        a.name = Some("t-fix-1234".into());
        assert_eq!(title(&a, Some(&tab), "Claude"), "Review PR");
        a.title = Some("Agent title".into());
        assert_eq!(title(&a, None, "Claude"), "Agent title");
        a.terminal_title_stripped = Some("Sidebar improvements".into());
        assert_eq!(title(&a, Some(&tab), "Claude"), "Sidebar improvements");
    }

    #[test]
    fn numeric_tab_labels_are_herdr_defaults_not_titles() {
        let a = agent("w1:p1", AgentStatus::Idle);
        let tab = TabInfo { tab_id: "w1:t1".into(), workspace_id: "w1".into(), label: "2".into() };
        assert_eq!(title(&a, Some(&tab), "Claude"), "New Claude thread");
    }

    #[test]
    fn shell_prompts_and_bare_agent_names_are_not_titles() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        a.terminal_title_stripped = Some("lucas@lucqs:~/Work/x".into());
        a.title = Some("Claude".into());
        a.name = Some("refactor auth".into());
        assert_eq!(title(&a, None, "Claude"), "refactor auth");
    }

    #[test]
    fn a_colon_in_a_real_title_is_kept() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        a.terminal_title_stripped = Some("Fix: login loops on mobile".into());
        assert_eq!(title(&a, None, "Claude"), "Fix: login loops on mobile");
        a.terminal_title_stripped = Some("Ask bob@example.com: about it".into());
        assert_eq!(title(&a, None, "Claude"), "Ask bob@example.com: about it");
    }

    #[test]
    fn counter_prefixes_and_whitespace_are_stripped() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        a.terminal_title_stripped = Some("  [12] Ship it  ".into());
        assert_eq!(title(&a, None, "Claude"), "Ship it");
        a.terminal_title_stripped = Some("[wip] Keep brackets".into());
        assert_eq!(title(&a, None, "Claude"), "[wip] Keep brackets");
        a.terminal_title_stripped = Some("   ".into());
        assert_eq!(title(&a, None, "Claude"), "New Claude thread");
    }

    #[test]
    fn harness_labels_prefer_herdr_display_names() {
        let mut a = agent("w1:p1", AgentStatus::Idle);
        assert_eq!(harness_label(&a), "Claude");
        a.agent = Some("opencode".into());
        assert_eq!(harness_label(&a), "OpenCode");
        a.agent = Some("newcli".into());
        assert_eq!(harness_label(&a), "Newcli");
        a.display_agent = Some("  Codex (work) ".into());
        assert_eq!(harness_label(&a), "Codex (work)");
        a.display_agent = Some(" ".into());
        a.agent = None;
        assert_eq!(harness_label(&a), "Agent");
    }

    #[test]
    fn project_and_branch_come_from_the_workspace_worktree_first() {
        let mut snapshot = SessionSnapshot::default();
        let mut a = agent("w1:p1", AgentStatus::Working);
        a.cwd = Some("/elsewhere".into());
        snapshot.agents.push(a);
        snapshot.workspaces.push(WorkspaceInfo {
            workspace_id: "w1".into(),
            label: "fix-login".into(),
            worktree: Some(WorktreeInfo {
                repo_name: "cockpit".into(),
                repo_root: "/w/cockpit".into(),
                checkout_path: "/h/.herdr/worktrees/cockpit/fix-login".into(),
                is_linked_worktree: true,
            }),
        });
        let looked_up = std::cell::RefCell::new(Vec::new());
        let checkout = |path: &Path| {
            looked_up.borrow_mut().push(path.to_path_buf());
            Some(Checkout { repo: "ignored".into(), branch: Some("fix-login".into()), root: path.into() })
        };
        let threads = build(&snapshot, &Activity::default(), &checkout);
        assert_eq!(threads[0].project, "cockpit");
        assert_eq!(threads[0].branch.as_deref(), Some("fix-login"));
        assert_eq!(looked_up.borrow().as_slice(), [Path::new("/h/.herdr/worktrees/cockpit/fix-login")]);
    }

    #[test]
    fn project_falls_back_to_git_then_directory_then_workspace_label() {
        let mut snapshot = SessionSnapshot::default();
        let mut with_cwd = agent("w1:p1", AgentStatus::Idle);
        with_cwd.foreground_cwd = Some("/w/api/src".into());
        with_cwd.cwd = Some("/w/ignored".into());
        snapshot.agents.push(with_cwd);
        let mut plain_dir = agent("w2:p1", AgentStatus::Idle);
        plain_dir.cwd = Some("/tmp/scratch".into());
        snapshot.agents.push(plain_dir);
        snapshot.agents.push(agent("w3:p1", AgentStatus::Idle));
        snapshot.workspaces.push(WorkspaceInfo { workspace_id: "w3".into(), label: "notes".into(), worktree: None });
        snapshot.agents.push(agent("w4:p1", AgentStatus::Idle));
        let checkout = |path: &Path| {
            (path == Path::new("/w/api/src"))
                .then(|| Checkout { repo: "api".into(), branch: Some("main".into()), root: "/w/api".into() })
        };
        let threads = build(&snapshot, &Activity::default(), &checkout);
        let by_pane = |pane: &str| threads.iter().find(|t| t.pane_id == pane).unwrap().clone();
        assert_eq!(by_pane("w1:p1").project, "api");
        assert_eq!(by_pane("w1:p1").branch.as_deref(), Some("main"));
        assert_eq!(by_pane("w2:p1").project, "scratch");
        assert_eq!(by_pane("w2:p1").branch, None);
        assert_eq!(by_pane("w3:p1").project, "notes");
        assert_eq!(by_pane("w4:p1").project, "no project");
    }

    #[test]
    fn first_observation_has_no_age_and_changes_are_timestamped() {
        let mut activity = Activity::default();
        activity.observe("a", AgentStatus::Working, at(10));
        assert_eq!(activity.changed_at("a"), None);
        activity.observe("a", AgentStatus::Working, at(20));
        assert_eq!(activity.changed_at("a"), None, "same status is not a change");
        activity.observe("a", AgentStatus::Done, at(30));
        assert_eq!(activity.changed_at("a"), Some(at(30)));
    }

    #[test]
    fn opening_a_finished_thread_shows_it_as_idle_until_it_changes_again() {
        let mut activity = Activity::default();
        activity.observe("a", AgentStatus::Working, at(1));
        activity.observe("a", AgentStatus::Done, at(2));
        assert_eq!(activity.effective("a", AgentStatus::Done), AgentStatus::Done);
        activity.mark_seen("a");
        assert_eq!(activity.effective("a", AgentStatus::Done), AgentStatus::Idle);
        activity.observe("a", AgentStatus::Working, at(3));
        assert_eq!(activity.effective("a", AgentStatus::Working), AgentStatus::Working);
        activity.observe("a", AgentStatus::Done, at(4));
        assert_eq!(activity.effective("a", AgentStatus::Done), AgentStatus::Done, "a new completion asks again");
    }

    #[test]
    fn seeing_a_thread_that_is_not_done_changes_nothing() {
        let mut activity = Activity::default();
        activity.observe("a", AgentStatus::Blocked, at(1));
        activity.mark_seen("a");
        assert_eq!(activity.effective("a", AgentStatus::Blocked), AgentStatus::Blocked);
        activity.observe("a", AgentStatus::Done, at(2));
        assert_eq!(activity.effective("a", AgentStatus::Done), AgentStatus::Done);
        activity.mark_seen("unknown-thread");
    }

    #[test]
    fn retain_forgets_threads_that_are_gone() {
        let mut activity = Activity::default();
        activity.observe("a", AgentStatus::Working, at(1));
        activity.observe("a", AgentStatus::Done, at(2));
        activity.retain(&HashSet::new());
        assert_eq!(activity.changed_at("a"), None);
    }

    fn named(id: &str, status: AgentStatus, changed: Option<u64>, seq: u64) -> Thread {
        Thread {
            id: id.into(),
            pane_id: id.into(),
            workspace_id: "w".into(),
            status,
            title: format!("title {id}"),
            project: "p".into(),
            branch: None,
            harness: "Claude".into(),
            changed_at: changed.map(at),
            change_seq: seq,
        }
    }

    #[test]
    fn sort_orders_by_group_then_recency_then_herdr_counter() {
        let mut threads = vec![
            named("idle", AgentStatus::Idle, Some(99), 0),
            named("working-old", AgentStatus::Working, Some(5), 0),
            named("blocked", AgentStatus::Blocked, None, 0),
            named("working-unknown-high", AgentStatus::Working, None, 9),
            named("working-new", AgentStatus::Working, Some(50), 0),
            named("working-unknown-low", AgentStatus::Working, None, 1),
            named("done", AgentStatus::Done, Some(1), 0),
            named("unknown", AgentStatus::Unknown, None, 0),
        ];
        sort(&mut threads);
        let ids: Vec<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            ["blocked", "done", "working-new", "working-old", "working-unknown-high", "working-unknown-low", "idle", "unknown"]
        );
    }

    #[test]
    fn sort_is_deterministic_for_identical_threads() {
        let mut a = vec![named("b", AgentStatus::Idle, None, 0), named("a", AgentStatus::Idle, None, 0)];
        let mut b = a.clone();
        b.reverse();
        sort(&mut a);
        sort(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn build_applies_activity_to_status_and_age() {
        let mut snapshot = SessionSnapshot::default();
        snapshot.agents.push(agent("w1:p1", AgentStatus::Done));
        let mut activity = Activity::default();
        activity.observe("w1:p1", AgentStatus::Working, at(1));
        activity.observe("w1:p1", AgentStatus::Done, at(2));
        activity.mark_seen("w1:p1");
        let threads = build(&snapshot, &activity, &no_checkout);
        assert_eq!(threads[0].status, AgentStatus::Idle);
        assert_eq!(threads[0].changed_at, Some(at(2)));
    }
}
