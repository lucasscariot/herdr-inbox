//! Pull requests for the threads' branches: one GitHub GraphQL query per
//! repository through `gh`, run on this machine whichever machine the thread
//! is on, and whether a PR monitor's background agent is repairing the PR.
//!
//! Parsing and classification are pure; [`fetch`] is the only I/O.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

/// A branch on GitHub: `(owner/name, branch)`.
pub type Key = (String, String);

/// How often the threads' pull requests are read again.
pub const INTERVAL: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(20);
/// Candidates per branch: a fork's PR from a same-named branch may be newer.
const CANDIDATES: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: PullState,
}

impl PullRequest {
    /// `#49 ready to merge`, as the sidebar shows it and the filter matches it.
    pub fn label(&self) -> String {
        format!("#{} {}", self.number, self.state.label())
    }
}

/// What a pull request is waiting for, the most pressing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullState {
    Merged,
    Closed,
    /// A PR monitor's background agent is working on it right now.
    AgentFixing,
    Draft,
    Conflicts,
    ChecksFailed,
    ChangesRequested,
    /// Unresolved review threads.
    Comments(usize),
    ChecksRunning,
    /// The base moved on and the repository wants the branch up to date.
    Behind,
    AwaitingReview,
    Ready,
    /// GitHub has not computed mergeability yet.
    Checking,
}

/// How a state reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Waiting on the user: merge it.
    Good,
    /// Something to read or answer.
    Attention,
    /// Broken: conflicts or failing checks.
    Bad,
    /// An agent is on it.
    Busy,
    Merged,
    /// Nothing to do yet.
    Quiet,
}

impl PullState {
    pub fn label(self) -> String {
        match self {
            PullState::Merged => "merged".into(),
            PullState::Closed => "closed".into(),
            PullState::AgentFixing => "agent fixing".into(),
            PullState::Draft => "draft".into(),
            PullState::Conflicts => "conflicts".into(),
            PullState::ChecksFailed => "checks failed".into(),
            PullState::ChangesRequested => "changes requested".into(),
            PullState::Comments(1) => "1 comment".into(),
            PullState::Comments(count) => format!("{count} comments"),
            PullState::ChecksRunning => "checks running".into(),
            PullState::Behind => "behind base".into(),
            PullState::AwaitingReview => "awaiting review".into(),
            PullState::Ready => "ready to merge".into(),
            PullState::Checking => "checking".into(),
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            PullState::Ready => Tone::Good,
            PullState::ChangesRequested | PullState::Comments(_) | PullState::Behind => Tone::Attention,
            PullState::Conflicts | PullState::ChecksFailed => Tone::Bad,
            PullState::AgentFixing => Tone::Busy,
            PullState::Merged => Tone::Merged,
            PullState::Closed
            | PullState::Draft
            | PullState::ChecksRunning
            | PullState::AwaitingReview
            | PullState::Checking => Tone::Quiet,
        }
    }
}

/// What GitHub says about a pull request, before classification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub draft: bool,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`.
    pub mergeable: String,
    /// `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, `UNSTABLE`, `HAS_HOOKS`, ...
    pub merge_state: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or none.
    pub review: Option<String>,
    pub unresolved: usize,
    /// The head commit's check rollup: `SUCCESS`, `FAILURE`, `PENDING`...
    pub checks: Option<String>,
    pub agent_fixing: bool,
}

/// The one thing a pull request is waiting for. A finished PR is only that;
/// an agent at work outranks what it is fixing; something broken outranks
/// what is merely pending.
pub fn classify(facts: &Facts) -> PullState {
    let checks = facts.checks.as_deref();
    match facts.state.as_str() {
        "MERGED" => return PullState::Merged,
        "CLOSED" => return PullState::Closed,
        _ => {}
    }
    if facts.agent_fixing {
        return PullState::AgentFixing;
    }
    if facts.draft {
        return PullState::Draft;
    }
    if facts.mergeable == "CONFLICTING" || facts.merge_state == "DIRTY" {
        return PullState::Conflicts;
    }
    if matches!(checks, Some("FAILURE" | "ERROR")) {
        return PullState::ChecksFailed;
    }
    if facts.review.as_deref() == Some("CHANGES_REQUESTED") {
        return PullState::ChangesRequested;
    }
    if facts.unresolved > 0 {
        return PullState::Comments(facts.unresolved);
    }
    if matches!(checks, Some("PENDING" | "EXPECTED")) {
        return PullState::ChecksRunning;
    }
    if facts.merge_state == "BEHIND" {
        return PullState::Behind;
    }
    if facts.review.as_deref() == Some("REVIEW_REQUIRED") {
        return PullState::AwaitingReview;
    }
    if facts.mergeable == "MERGEABLE" {
        return PullState::Ready;
    }
    PullState::Checking
}

/// One query for every branch of a repository, each under its own alias.
pub fn query(repo: &str, branches: &[String]) -> Option<String> {
    let (owner, name) = repo.split_once('/')?;
    let mut fields = String::new();
    for (index, branch) in branches.iter().enumerate() {
        fields.push_str(&format!(
            "b{index}: pullRequests(headRefName: {branch}, first: {CANDIDATES}, \
             orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ \
             number url state isDraft mergeable mergeStateStatus reviewDecision \
             headRepository {{ nameWithOwner }} \
             reviewThreads(first: 100) {{ nodes {{ isResolved }} }} \
             commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }} }} }} ",
            branch = Value::from(branch.as_str()),
        ));
    }
    Some(format!("query {{ repository(owner: {}, name: {}) {{ {fields}}} }}", Value::from(owner), Value::from(name)))
}

/// Each branch's newest pull request opened from this repository, from the
/// answer to [`query`]. `fixing` says whether a monitor is repairing a PR.
pub fn parse(
    repo: &str,
    branches: &[String],
    answer: &str,
    fixing: impl Fn(u64) -> bool,
) -> Result<HashMap<String, PullRequest>, String> {
    let answer: Value = serde_json::from_str(answer).map_err(|err| format!("unreadable answer from GitHub: {err}"))?;
    if let Some(message) = answer["errors"][0]["message"].as_str() {
        return Err(message.to_string());
    }
    let repository = &answer["data"]["repository"];
    if repository.is_null() {
        return Err(format!("{repo} is not on GitHub or not readable"));
    }
    let mut found = HashMap::new();
    for (index, branch) in branches.iter().enumerate() {
        let nodes = repository[format!("b{index}")]["nodes"].as_array().cloned().unwrap_or_default();
        let ours = nodes.into_iter().find(|node| {
            node["headRepository"]["nameWithOwner"].as_str().is_some_and(|head| head.eq_ignore_ascii_case(repo))
        });
        let Some(node) = ours else { continue };
        let Some(number) = node["number"].as_u64() else { continue };
        let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
        let unresolved = node["reviewThreads"]["nodes"]
            .as_array()
            .map_or(0, |threads| threads.iter().filter(|t| t["isResolved"] == false).count());
        let facts = Facts {
            state: text(&node["state"]),
            draft: node["isDraft"].as_bool().unwrap_or(false),
            mergeable: text(&node["mergeable"]),
            merge_state: text(&node["mergeStateStatus"]),
            review: node["reviewDecision"].as_str().map(str::to_string),
            unresolved,
            checks: node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str().map(str::to_string),
            agent_fixing: fixing(number),
        };
        found.insert(branch.clone(), PullRequest { number, url: text(&node["url"]), state: classify(&facts) });
    }
    Ok(found)
}

/// Reads a repository's pull requests for these branches with `gh`.
pub fn fetch(repo: &str, branches: &[String]) -> Result<HashMap<String, PullRequest>, String> {
    let query = query(repo, branches).ok_or_else(|| format!("{repo} is not owner/name"))?;
    let mut command = Command::new("gh");
    command.args(["api", "graphql", "-f"]).arg(format!("query={query}"));
    let answer = crate::herdr::transport::run_with_timeout(command, TIMEOUT).map_err(|err| err.to_string())?;
    let monitors = monitor_dir();
    parse(repo, branches, &answer, |number| monitors.as_deref().is_some_and(|dir| repairing(dir, repo, number)))
}

/// Where the change-to-pr skill's `pr_watch.py` keeps one directory per PR.
fn monitor_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(state.join("personal-skills/pr-watch"))
}

/// Whether a live monitor is running its agent on this PR. Its status file
/// says `repairing` with the supervisor's pid; a dead pid is a stale file.
pub fn repairing(dir: &Path, repo: &str, number: u64) -> bool {
    let name = format!("{}-{number}", repo.to_lowercase().replace('/', "%2F"));
    let Ok(text) = std::fs::read_to_string(dir.join(name).join("status.json")) else {
        return false;
    };
    let Ok(status) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    status["phase"] == "repairing" && status["pid"].as_i64().is_some_and(alive)
}

fn alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // Signal 0 checks existence; EPERM means it exists under another user.
    // SAFETY: kill with signal 0 sends nothing.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Facts {
        Facts { state: "OPEN".into(), mergeable: "MERGEABLE".into(), merge_state: "CLEAN".into(), ..Facts::default() }
    }

    #[test]
    fn a_clean_open_pull_request_is_ready_to_merge() {
        assert_eq!(classify(&open()), PullState::Ready);
        let approved = Facts { review: Some("APPROVED".into()), checks: Some("SUCCESS".into()), ..open() };
        assert_eq!(classify(&approved), PullState::Ready);
    }

    #[test]
    fn finished_pull_requests_say_only_that() {
        let merged = Facts { state: "MERGED".into(), checks: Some("FAILURE".into()), agent_fixing: true, ..open() };
        assert_eq!(classify(&merged), PullState::Merged);
        assert_eq!(classify(&Facts { state: "CLOSED".into(), ..open() }), PullState::Closed);
    }

    #[test]
    fn the_most_pressing_problem_wins() {
        let everything = Facts {
            mergeable: "CONFLICTING".into(),
            checks: Some("FAILURE".into()),
            review: Some("CHANGES_REQUESTED".into()),
            unresolved: 2,
            ..open()
        };
        assert_eq!(classify(&Facts { agent_fixing: true, ..everything.clone() }), PullState::AgentFixing);
        assert_eq!(classify(&everything), PullState::Conflicts);
        let failing = Facts { mergeable: "MERGEABLE".into(), ..everything };
        assert_eq!(classify(&failing), PullState::ChecksFailed);
        let reviewed = Facts { checks: Some("PENDING".into()), ..failing };
        assert_eq!(classify(&reviewed), PullState::ChangesRequested);
        let commented = Facts { review: None, ..reviewed };
        assert_eq!(classify(&commented), PullState::Comments(2));
        assert_eq!(classify(&Facts { unresolved: 0, ..commented }), PullState::ChecksRunning);
    }

    #[test]
    fn waiting_states_come_after_problems() {
        assert_eq!(classify(&Facts { draft: true, checks: Some("FAILURE".into()), ..open() }), PullState::Draft);
        assert_eq!(classify(&Facts { merge_state: "BEHIND".into(), ..open() }), PullState::Behind);
        let review = Facts { review: Some("REVIEW_REQUIRED".into()), merge_state: "BLOCKED".into(), ..open() };
        assert_eq!(classify(&review), PullState::AwaitingReview);
        assert_eq!(classify(&Facts { mergeable: "UNKNOWN".into(), ..open() }), PullState::Checking);
    }

    #[test]
    fn labels_read_as_a_sentence() {
        let pull = PullRequest { number: 49, url: String::new(), state: PullState::Ready };
        assert_eq!(pull.label(), "#49 ready to merge");
        assert_eq!(PullState::Comments(1).label(), "1 comment");
        assert_eq!(PullState::Comments(3).label(), "3 comments");
    }

    #[test]
    fn the_query_names_every_branch_and_escapes_it() {
        let query = query("lucas/inbox", &["fix-login".into(), "we\"ird".into()]).unwrap();
        assert!(query.starts_with(r#"query { repository(owner: "lucas", name: "inbox") {"#), "{query}");
        assert!(query.contains(r#"b0: pullRequests(headRefName: "fix-login""#));
        assert!(query.contains(r#"b1: pullRequests(headRefName: "we\"ird""#));
        assert_eq!(super::query("no-slash", &[]), None);
    }

    fn node(number: u64, head: &str, state: &str) -> Value {
        serde_json::json!({
            "number": number, "url": format!("https://github.com/{head}/pull/{number}"), "state": state,
            "isDraft": false, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "reviewDecision": null,
            "headRepository": {"nameWithOwner": head},
            "reviewThreads": {"nodes": [{"isResolved": true}, {"isResolved": false}]},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]},
        })
    }

    #[test]
    fn each_branch_gets_its_newest_pull_request_from_this_repository() {
        let answer = serde_json::json!({"data": {"repository": {
            "b0": {"nodes": [node(9, "fork/inbox", "OPEN"), node(7, "Lucas/Inbox", "OPEN")]},
            "b1": {"nodes": []},
            "b2": {"nodes": [node(3, "lucas/inbox", "MERGED")]},
        }}});
        let branches = ["fix".to_string(), "none".to_string(), "old".to_string()];
        let found = parse("lucas/inbox", &branches, &answer.to_string(), |number| number == 3).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found["fix"].number, 7, "a fork's same-named branch is not ours");
        assert_eq!(found["fix"].state, PullState::Comments(1));
        assert_eq!(found["fix"].url, "https://github.com/Lucas/Inbox/pull/7");
        assert_eq!(found["old"].state, PullState::Merged);
    }

    #[test]
    fn a_monitor_agent_on_the_pull_request_is_shown() {
        let answer = serde_json::json!({"data": {"repository": {"b0": {"nodes": [node(7, "lucas/inbox", "OPEN")]}}}});
        let found = parse("lucas/inbox", &["fix".into()], &answer.to_string(), |number| number == 7).unwrap();
        assert_eq!(found["fix"].state, PullState::AgentFixing);
    }

    #[test]
    fn errors_from_github_are_reported() {
        let denied = r#"{"data": {"repository": null}, "errors": [{"message": "Could not resolve to a Repository"}]}"#;
        assert_eq!(parse("a/b", &[], denied, |_| false), Err("Could not resolve to a Repository".into()));
        assert!(parse("a/b", &[], r#"{"data": {"repository": null}}"#, |_| false).is_err());
        assert!(parse("a/b", &[], "gh: not logged in", |_| false).is_err());
    }

    #[test]
    fn only_a_live_monitor_that_is_repairing_counts() {
        let dir = tempfile::tempdir().unwrap();
        let write = |number: u64, phase: &str, pid: i64| {
            let path = dir.path().join(format!("lucas%2Finbox-{number}"));
            std::fs::create_dir_all(&path).unwrap();
            let status = serde_json::json!({"phase": phase, "pid": pid});
            std::fs::write(path.join("status.json"), status.to_string()).unwrap();
        };
        let me = std::process::id() as i64;
        write(1, "repairing", me);
        write(2, "waiting", me);
        write(3, "repairing", i64::from(i32::MAX));
        assert!(repairing(dir.path(), "Lucas/Inbox", 1), "the directory name is lower case");
        assert!(!repairing(dir.path(), "lucas/inbox", 2));
        assert!(!repairing(dir.path(), "lucas/inbox", 3), "a stale status file is ignored");
        assert!(!repairing(dir.path(), "lucas/inbox", 4));
    }
}
