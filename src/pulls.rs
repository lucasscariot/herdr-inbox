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
/// Review threads per page: GitHub's maximum.
const PAGE: usize = 100;
/// Pages of review threads read after the first before the count stays a
/// floor; a guard against an answer that never ends.
const MAX_PAGES: usize = 20;

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
    /// A rule on GitHub holds the merge for a reason the sidebar cannot
    /// name: a required deployment, signing, a merge queue, a required check
    /// that never ran...
    Blocked,
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
            PullState::Blocked => "blocked".into(),
            PullState::Ready => "ready to merge".into(),
            PullState::Checking => "checking".into(),
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            PullState::Ready => Tone::Good,
            PullState::ChangesRequested | PullState::Comments(_) | PullState::Behind | PullState::Blocked => {
                Tone::Attention
            }
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
    // `mergeable` only says there is no conflict; a rule may still hold it.
    if facts.merge_state == "BLOCKED" {
        return PullState::Blocked;
    }
    if facts.mergeable == "MERGEABLE" {
        return PullState::Ready;
    }
    PullState::Checking
}

/// One page of review threads, from the start or after a cursor.
fn threads_field(after: Option<&str>) -> String {
    let after = after.map(|cursor| format!(", after: {}", Value::from(cursor))).unwrap_or_default();
    format!("reviewThreads(first: {PAGE}{after}) {{ nodes {{ isResolved }} pageInfo {{ hasNextPage endCursor }} }}")
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
             headRepository {{ nameWithOwner }} {threads} \
             commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }} }} }} ",
            branch = Value::from(branch.as_str()),
            threads = threads_field(None),
        ));
    }
    Some(format!("query {{ repository(owner: {}, name: {}) {{ {fields}}} }}", Value::from(owner), Value::from(name)))
}

/// The page of a pull request's review threads after `after`.
pub fn threads_query(repo: &str, number: u64, after: &str) -> Option<String> {
    let (owner, name) = repo.split_once('/')?;
    Some(format!(
        "query {{ repository(owner: {}, name: {}) {{ pullRequest(number: {number}) {{ {} }} }} }}",
        Value::from(owner),
        Value::from(name),
        threads_field(Some(after)),
    ))
}

/// A pull request as GitHub answered, before classification. `more_threads`
/// is the cursor after the first page of review threads when there are more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub number: u64,
    pub url: String,
    pub facts: Facts,
    pub more_threads: Option<String>,
}

impl Answer {
    /// The pull request as the sidebar shows it; `fixing` says whether a
    /// monitor's agent is repairing it.
    pub fn classify(mut self, fixing: bool) -> PullRequest {
        self.facts.agent_fixing = fixing;
        PullRequest { number: self.number, url: self.url, state: classify(&self.facts) }
    }
}

/// Each branch's newest pull request opened from this repository, from the
/// answer to [`query`].
pub fn parse(repo: &str, branches: &[String], answer: &str) -> Result<HashMap<String, Answer>, String> {
    let answer = data(answer)?;
    let repository = &answer["repository"];
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
        let (unresolved, more_threads) = threads(&node["reviewThreads"]);
        let facts = Facts {
            state: text(&node["state"]),
            draft: node["isDraft"].as_bool().unwrap_or(false),
            mergeable: text(&node["mergeable"]),
            merge_state: text(&node["mergeStateStatus"]),
            review: node["reviewDecision"].as_str().map(str::to_string),
            unresolved,
            checks: node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str().map(str::to_string),
            agent_fixing: false,
        };
        found.insert(branch.clone(), Answer { number, url: text(&node["url"]), facts, more_threads });
    }
    Ok(found)
}

/// Unresolved threads on the page answering [`threads_query`], and the
/// cursor of the page after it when there is one.
pub fn parse_threads(answer: &str) -> Result<(usize, Option<String>), String> {
    let answer = data(answer)?;
    let page = &answer["repository"]["pullRequest"]["reviewThreads"];
    if page.is_null() {
        return Err("GitHub answered without the review threads".into());
    }
    Ok(threads(page))
}

/// A GraphQL answer's `data`, or its first error.
fn data(answer: &str) -> Result<Value, String> {
    let mut answer: Value =
        serde_json::from_str(answer).map_err(|err| format!("unreadable answer from GitHub: {err}"))?;
    if let Some(message) = answer["errors"][0]["message"].as_str() {
        return Err(message.to_string());
    }
    Ok(answer["data"].take())
}

fn threads(page: &Value) -> (usize, Option<String>) {
    let unresolved =
        page["nodes"].as_array().map_or(0, |nodes| nodes.iter().filter(|t| t["isResolved"] == false).count());
    let info = &page["pageInfo"];
    let next = if info["hasNextPage"] == true { info["endCursor"].as_str().map(str::to_string) } else { None };
    (unresolved, next)
}

/// Reads a repository's pull requests for these branches with `gh`. An open
/// pull request with more than a page of review threads costs one more
/// query per page, so none of its unresolved threads is missed.
pub fn fetch(repo: &str, branches: &[String]) -> Result<HashMap<String, PullRequest>, String> {
    let query = query(repo, branches).ok_or_else(|| format!("{repo} is not owner/name"))?;
    let answers = parse(repo, branches, &graphql(&query)?)?;
    let monitors = monitor_dir();
    let mut found = HashMap::new();
    for (branch, mut answer) in answers {
        if answer.facts.state == "OPEN" {
            let mut cursor = answer.more_threads.take();
            for _ in 0..MAX_PAGES {
                let Some(after) = cursor.take() else { break };
                let query =
                    threads_query(repo, answer.number, &after).ok_or_else(|| format!("{repo} is not owner/name"))?;
                let (unresolved, next) = parse_threads(&graphql(&query)?)?;
                answer.facts.unresolved += unresolved;
                cursor = next;
            }
        }
        let fixing = monitors.as_deref().is_some_and(|dir| repairing(dir, repo, answer.number));
        found.insert(branch, answer.classify(fixing));
    }
    Ok(found)
}

fn graphql(query: &str) -> Result<String, String> {
    let mut command = Command::new("gh");
    command.args(["api", "graphql", "-f"]).arg(format!("query={query}"));
    crate::herdr::transport::run_with_timeout(command, TIMEOUT).map_err(|err| err.to_string())
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
    fn a_rule_holding_the_merge_is_not_ready() {
        // Mergeable only means no conflict: a required deployment, signing
        // or a check that never ran still blocks the merge.
        let blocked = Facts { merge_state: "BLOCKED".into(), ..open() };
        assert_eq!(classify(&blocked), PullState::Blocked);
        let approved = Facts { review: Some("APPROVED".into()), checks: Some("SUCCESS".into()), ..blocked.clone() };
        assert_eq!(classify(&approved), PullState::Blocked);
        assert_eq!(PullState::Blocked.tone(), Tone::Attention);
        // What the sidebar can name comes first.
        assert_eq!(classify(&Facts { checks: Some("PENDING".into()), ..blocked.clone() }), PullState::ChecksRunning);
        assert_eq!(classify(&Facts { unresolved: 1, ..blocked }), PullState::Comments(1));
    }

    #[test]
    fn labels_read_as_a_sentence() {
        let pull = PullRequest { number: 49, url: String::new(), state: PullState::Ready };
        assert_eq!(pull.label(), "#49 ready to merge");
        assert_eq!(PullState::Comments(1).label(), "1 comment");
        assert_eq!(PullState::Comments(3).label(), "3 comments");
        assert_eq!(PullState::Blocked.label(), "blocked");
    }

    #[test]
    fn the_query_names_every_branch_and_escapes_it() {
        let query = query("lucas/inbox", &["fix-login".into(), "we\"ird".into()]).unwrap();
        assert!(query.starts_with(r#"query { repository(owner: "lucas", name: "inbox") {"#), "{query}");
        assert!(query.contains(r#"b0: pullRequests(headRefName: "fix-login""#));
        assert!(query.contains(r#"b1: pullRequests(headRefName: "we\"ird""#));
        assert!(query.contains("reviewThreads(first: 100) { nodes { isResolved } pageInfo { hasNextPage endCursor }"));
        assert_eq!(super::query("no-slash", &[]), None);
    }

    #[test]
    fn the_next_page_of_threads_is_asked_after_its_cursor() {
        let query = threads_query("lucas/inbox", 49, "Y3Vyc29y\"1").unwrap();
        assert!(query.starts_with(r#"query { repository(owner: "lucas", name: "inbox") { pullRequest(number: 49) {"#));
        assert!(query.contains(r#"reviewThreads(first: 100, after: "Y3Vyc29y\"1") { nodes { isResolved }"#));
        assert_eq!(threads_query("no-slash", 1, "c"), None);
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
        let found = parse("lucas/inbox", &branches, &answer.to_string()).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found["fix"].number, 7, "a fork's same-named branch is not ours");
        assert_eq!(found["fix"].facts.unresolved, 1);
        assert_eq!(found["fix"].more_threads, None);
        assert_eq!(found["fix"].url, "https://github.com/Lucas/Inbox/pull/7");
        let fix = found["fix"].clone();
        assert_eq!(fix.clone().classify(false).state, PullState::Comments(1));
        assert_eq!(fix.classify(true).state, PullState::AgentFixing, "a monitor's agent on it is shown");
        assert_eq!(found["old"].clone().classify(false).state, PullState::Merged);
    }

    #[test]
    fn review_threads_past_the_first_page_are_counted_too() {
        let mut first = node(7, "lucas/inbox", "OPEN");
        first["reviewThreads"]["pageInfo"] = serde_json::json!({"hasNextPage": true, "endCursor": "c1"});
        let answer = serde_json::json!({"data": {"repository": {"b0": {"nodes": [first]}}}});
        let found = parse("lucas/inbox", &["fix".into()], &answer.to_string()).unwrap();
        assert_eq!(found["fix"].more_threads.as_deref(), Some("c1"));
        assert_eq!(found["fix"].facts.unresolved, 1);

        let page = |resolved: &[bool], next: Option<&str>| {
            let nodes: Vec<Value> = resolved.iter().map(|r| serde_json::json!({"isResolved": r})).collect();
            let info = serde_json::json!({"hasNextPage": next.is_some(), "endCursor": next});
            let threads = serde_json::json!({"nodes": nodes, "pageInfo": info});
            let answer = serde_json::json!({"data": {"repository": {"pullRequest": {"reviewThreads": threads}}}});
            answer.to_string()
        };
        assert_eq!(parse_threads(&page(&[false, true, false], Some("c2"))), Ok((2, Some("c2".into()))));
        assert_eq!(parse_threads(&page(&[true], None)), Ok((0, None)));
        let last = serde_json::json!({"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": []}}}}});
        assert_eq!(parse_threads(&last.to_string()), Ok((0, None)), "no page info means no next page");
        assert!(parse_threads(r#"{"data": {"repository": {"pullRequest": null}}}"#).is_err());
    }

    #[test]
    fn errors_from_github_are_reported() {
        let denied = r#"{"data": {"repository": null}, "errors": [{"message": "Could not resolve to a Repository"}]}"#;
        assert_eq!(parse("a/b", &[], denied), Err("Could not resolve to a Repository".into()));
        assert!(parse("a/b", &[], r#"{"data": {"repository": null}}"#).is_err());
        assert!(parse("a/b", &[], "gh: not logged in").is_err());
        assert_eq!(parse_threads(denied), Err("Could not resolve to a Repository".into()));
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
