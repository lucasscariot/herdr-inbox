//! Starting a thread: one workspace (a new git worktree or an existing
//! checkout), an agent started in it, and the task sent once it is ready.
//!
//! Every step runs the `herdr` CLI on the target machine, which waits for the
//! agent's readiness the way Herdr intends. A journal is written before the
//! first change and after every step, so an interrupted launch is never
//! replayed blindly.

use std::process::Stdio;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Effective;
use crate::discovery::{Catalog, Project};
use crate::herdr::api::parse_response;
use crate::herdr::transport::Runner;

/// Herdr's error when an agent is waiting at a trust, login or update dialog.
const STARTUP_BLOCKED: &str = "blocked during startup";
/// Herdr accepted the prompt but saw no state change within its window.
const PROMPT_STALLED: &str = "agent_prompt_stalled";
const TITLE_LIMIT: usize = 72;
const BRANCH_SLUG_LIMIT: usize = 32;
const FILLER: &[&str] = &["a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "with", "please", "can", "you"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceChoice {
    /// A new git worktree; the branch is derived from the task unless named.
    NewWorktree { branch: Option<String> },
    /// An existing checkout of the project.
    Checkout { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub machine_id: String,
    pub machine_label: String,
    pub project: Project,
    pub harness: String,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub task: String,
    pub workspace: WorkspaceChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Creating,
    Created,
    Starting,
    /// The agent waits at a startup dialog; the task follows once it is idle.
    StartupBlocked,
    Ready,
    Submitting,
    Submitted,
    NeedsAttention,
}

/// The journal of one launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub machine_id: String,
    pub machine_label: String,
    pub project: String,
    pub repo: String,
    pub harness: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub thinking: String,
    pub title: String,
    pub task: String,
    pub agent_name: String,
    pub created_at: u64,
    pub stage: Stage,
    /// `worktree` or `checkout`.
    pub workspace: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// Sent, but Herdr saw no reaction in time; never replayed.
    #[serde(default)]
    pub unverified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_stage: Option<Stage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What `plan` decided, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub record: Record,
    /// Arguments passed to the agent CLI after `--`.
    pub agent_args: Vec<String>,
    pub base: Option<String>,
    /// For `herdr agent start --timeout`.
    pub start_timeout_ms: u64,
}

/// A thread title: the task on one line, at most 72 characters.
pub fn task_title(task: &str) -> String {
    let collapsed = task.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(TITLE_LIMIT).collect()
}

/// A short, unique branch name from the task's first line.
pub fn branch_name(task: &str, taken: &[&str], prefix: &str) -> String {
    let first = task.trim().lines().next().unwrap_or("").to_lowercase();
    let words: Vec<&str> = first.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut slug = String::new();
    for word in words.into_iter().filter(|w| !FILLER.contains(w)) {
        let candidate = if slug.is_empty() { word.to_string() } else { format!("{slug}-{word}") };
        if candidate.len() > BRANCH_SLUG_LIMIT && !slug.is_empty() {
            break;
        }
        slug = candidate;
    }
    let slug: String = if slug.is_empty() { "thread".into() } else { slug.chars().take(40).collect() };
    let slug = slug.trim_matches('-').to_string();
    let mut name = format!("{prefix}{slug}");
    let mut suffix = 2;
    while taken.contains(&name.as_str()) {
        name = format!("{prefix}{slug}-{suffix}");
        suffix += 1;
    }
    name
}

/// Git's rules for branch names, the ones a typed name can break.
pub fn validate_branch(branch: &str) -> Result<(), String> {
    let first_ok = branch.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let chars_ok = branch.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    let bad = branch.ends_with(".lock")
        || branch.ends_with('/')
        || branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{");
    if first_ok && chars_ok && !bad {
        Ok(())
    } else {
        Err(format!("Branch names use letters, digits, dots, dashes, and slashes: {branch}"))
    }
}

/// Removes every occurrence of a flag and its value from `args`.
fn strip_flag(args: Vec<String>, names: &[&str]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if names.contains(&arg.as_str()) {
            skip = true;
            continue;
        }
        if names.iter().any(|name| arg.starts_with(&format!("{name}="))) {
            continue;
        }
        kept.push(arg);
    }
    kept
}

/// Validates a request and decides every argument, before anything changes.
pub fn plan(
    request: &Request,
    settings: &Effective,
    catalog: Option<&Catalog>,
    id: String,
    now: SystemTime,
) -> Result<Plan, String> {
    let task = request.task.trim();
    if task.is_empty() {
        return Err("Write a task before launching.".into());
    }
    let (workspace, branch, cwd) = match &request.workspace {
        WorkspaceChoice::NewWorktree { branch } => {
            let taken = request.project.taken_branches();
            let branch = match branch.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                Some(named) => {
                    if taken.contains(&named) {
                        return Err(format!(
                            "Branch {named} already has a worktree. Choose it from the Workspace list instead."
                        ));
                    }
                    named.to_string()
                }
                None => branch_name(task, &taken, &settings.branch_prefix),
            };
            validate_branch(&branch)?;
            ("worktree", branch, String::new())
        }
        WorkspaceChoice::Checkout { path } => {
            let checkout = request.project.checkouts.iter().find(|c| &c.path == path).cloned();
            let checkout = checkout.unwrap_or_else(|| request.project.main_checkout());
            ("checkout", checkout.branch, checkout.path)
        }
    };
    let catalog = catalog.cloned().unwrap_or_default();
    let mut args = settings.harness_args.get(&request.harness).cloned().unwrap_or_default();
    let model = request.model.clone().filter(|m| !m.trim().is_empty());
    if let Some(model) = &model {
        if !catalog.selectable {
            return Err(format!("{} does not support choosing a model at launch", request.harness));
        }
        // The composer's choice wins over a model pinned in harness_args.
        args = strip_flag(args, &["--model", "-m"]);
        args.extend(["--model".to_string(), model.clone()]);
    }
    let thinking = request.thinking.clone().filter(|t| !t.trim().is_empty());
    if let Some(level) = &thinking {
        let flag = catalog.thinking_flag.as_str();
        if !matches!(flag, "--thinking" | "--effort") || !catalog.thinking.contains(level) {
            return Err(format!("{} does not support thinking level {level}", request.harness));
        }
        args = strip_flag(args, &[flag]);
        args.extend([flag.to_string(), level.clone()]);
    }
    let title = task_title(task);
    let slug: String = title
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(16)
        .collect();
    let slug =
        if slug.trim_matches('-').is_empty() { "thread".to_string() } else { slug.trim_matches('-').to_string() };
    let record = Record {
        agent_name: format!("t-{slug}-{}", &id[..id.len().min(8)]),
        machine_id: request.machine_id.clone(),
        machine_label: request.machine_label.clone(),
        project: request.project.name.clone(),
        repo: request.project.path.clone(),
        harness: request.harness.clone(),
        model: model.unwrap_or_default(),
        thinking: thinking.unwrap_or_default(),
        title,
        task: task.to_string(),
        created_at: crate::discovery::seconds(now),
        stage: Stage::Creating,
        workspace: workspace.into(),
        branch,
        cwd,
        workspace_id: None,
        pane_id: None,
        tab_id: None,
        unverified: false,
        failed_stage: None,
        error: None,
        id,
    };
    Ok(Plan { record, agent_args: args, base: None, start_timeout_ms: settings.agent_start_timeout_ms })
}

/// A `herdr` CLI failure: Herdr's error code when it gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Runs `herdr <args>` on a machine and returns the JSON `result`. Herdr
/// prints errors as JSON on stdout too; stderr is only the fallback.
pub fn herdr(runner: &Runner, args: &[&str], timeout: Duration) -> Result<Value, CliError> {
    let mut command = runner.command(args);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run(command, timeout)?;
    let stdout = if runner.has_marker() {
        crate::herdr::ssh::after_marker(&output.stdout).unwrap_or("").to_string()
    } else {
        output.stdout
    };
    // Herdr prints its JSON answer on stdout, but some commands print their
    // JSON error on stderr: read whichever has one.
    let json = |text: &str| text.lines().rev().find(|l| l.trim_start().starts_with('{')).map(str::to_string);
    match json(&stdout).or_else(|| json(&output.stderr)) {
        Some(line) => parse_response(&line)
            .map_err(|err| CliError { code: err.code().map(str::to_string), message: err.to_string() }),
        None if output.success => Ok(Value::Null),
        None => Err(CliError {
            code: None,
            message: output.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("herdr failed").trim().into(),
        }),
    }
}

struct Output {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run(mut command: std::process::Command, timeout: Duration) -> Result<Output, CliError> {
    use std::io::Read;
    let fail = |message: String| CliError { code: None, message };
    let mut child = command.spawn().map_err(|err| fail(format!("cannot run herdr: {err}")))?;
    let mut stdout = child.stdout.take().ok_or_else(|| fail("no stdout".into()))?;
    let mut stderr = child.stderr.take().ok_or_else(|| fail("no stderr".into()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let mut err = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = stderr.read_to_string(&mut err);
        let _ = tx.send((out, err));
    });
    let read = rx.recv_timeout(timeout);
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|err| fail(err.to_string()))?;
    let (stdout, stderr) = read.map_err(|_| fail("herdr did not answer in time".into()))?;
    Ok(Output { stdout, stderr, success: status.success() })
}

/// A launch that stopped, with its journal and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub record: Box<Record>,
    pub error: String,
}

impl Failure {
    fn new(record: Record, error: String) -> Self {
        Self { record: Box::new(record), error }
    }
}

/// How a launch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The task reached the agent (maybe unverified, see the record).
    Sent(Record),
    /// The agent waits at a startup dialog; the task follows once it is idle.
    WaitingForStartup(Record),
}

/// Runs a plan. `journal` is called before the first change and after every
/// step; `progress` gets a short sentence for the user.
pub fn execute(
    runner: &Runner,
    plan: Plan,
    journal: &dyn Fn(&Record),
    progress: &dyn Fn(&str),
) -> Result<Outcome, Failure> {
    let mut record = plan.record.clone();
    journal(&record);
    let result = steps(runner, &mut record, &plan, journal, progress);
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            record.failed_stage = Some(record.stage);
            record.stage = Stage::NeedsAttention;
            record.error = Some(error.clone());
            journal(&record);
            Err(Failure::new(record, error))
        }
    }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(value, |value, key| value.get(key))?.as_str()
}

fn steps(
    runner: &Runner,
    record: &mut Record,
    plan: &Plan,
    journal: &dyn Fn(&Record),
    progress: &dyn Fn(&str),
) -> Result<Outcome, String> {
    let (agent_args, base) = (&plan.agent_args, plan.base.as_deref());
    let created = if record.workspace == "worktree" {
        progress(&format!("creating worktree {}", record.branch));
        let mut args = vec![
            "worktree",
            "create",
            "--cwd",
            &record.repo,
            "--branch",
            &record.branch,
            "--label",
            &record.project,
            "--no-focus",
        ];
        if let Some(base) = base {
            args.extend(["--base", base]);
        }
        herdr(runner, &args, Duration::from_secs(60)).map_err(|e| e.message)?
    } else {
        progress("creating the workspace");
        herdr(
            runner,
            &["workspace", "create", "--cwd", &record.cwd, "--label", &record.project, "--no-focus"],
            Duration::from_secs(30),
        )
        .map_err(|e| e.message)?
    };
    record.workspace_id = str_at(&created, &["workspace", "workspace_id"]).map(str::to_string);
    record.pane_id = str_at(&created, &["root_pane", "pane_id"]).map(str::to_string);
    record.tab_id = str_at(&created, &["tab", "tab_id"]).map(str::to_string);
    if record.workspace == "worktree" {
        record.cwd = str_at(&created, &["workspace", "worktree", "checkout_path"])
            .or_else(|| str_at(&created, &["worktree", "path"]))
            .or_else(|| str_at(&created, &["root_pane", "cwd"]))
            .unwrap_or_default()
            .to_string();
    }
    let pane = record.pane_id.clone().ok_or("Herdr did not say which pane it created")?;
    record.stage = Stage::Created;
    journal(record);
    if let Some(tab) = record.tab_id.clone() {
        // Cosmetic: a failed rename never fails the launch.
        let _ = herdr(runner, &["tab", "rename", &tab, &record.title], Duration::from_secs(15));
    }

    progress(&format!("starting {}", record.harness));
    record.stage = Stage::Starting;
    journal(record);
    let timeout = plan.start_timeout_ms.to_string();
    let mut start: Vec<&str> =
        vec!["agent", "start", &record.agent_name, "--kind", &record.harness, "--pane", &pane, "--timeout", &timeout];
    if !agent_args.is_empty() {
        start.push("--");
        start.extend(agent_args.iter().map(String::as_str));
    }
    let started = herdr(runner, &start, Duration::from_millis(plan.start_timeout_ms) + Duration::from_secs(25));
    let label = crate::threads::harness_label_for(&record.harness);
    let metadata = |record: &Record| {
        let token = format!("thread={}", record.title);
        let _ = herdr(
            runner,
            &[
                "pane",
                "report-metadata",
                &pane,
                "--source",
                "herdr-inbox",
                "--display-agent",
                &label,
                "--token",
                &token,
            ],
            Duration::from_secs(15),
        );
    };
    if let Err(error) = started {
        if !error.message.contains(STARTUP_BLOCKED) {
            return Err(error.message);
        }
        // A trust, login or update dialog waits in the terminal. Keep the
        // task: it is sent as soon as the user answers and the agent is idle.
        record.stage = Stage::StartupBlocked;
        journal(record);
        metadata(record);
        progress("waiting for the agent's startup prompt");
        return Ok(Outcome::WaitingForStartup(record.clone()));
    }
    record.stage = Stage::Ready;
    journal(record);
    metadata(record);
    progress("sending the task");
    submit(runner, record, journal)?;
    Ok(Outcome::Sent(record.clone()))
}

/// Sends the task and records the result.
fn submit(runner: &Runner, record: &mut Record, journal: &dyn Fn(&Record)) -> Result<(), String> {
    let pane = record.pane_id.clone().ok_or("the launch has no pane")?;
    record.stage = Stage::Submitting;
    journal(record);
    let prompt = [
        "agent",
        "prompt",
        &pane,
        &record.task,
        "--wait",
        "--until",
        "working",
        "--until",
        "blocked",
        "--timeout",
        "15000",
    ];
    match herdr(runner, &prompt, Duration::from_secs(30)) {
        Ok(_) => {}
        // Herdr typed the task but saw no state change in time: a fast answer
        // or a slow first turn. Never replay it; just say so.
        Err(error) if error.code.as_deref() == Some(PROMPT_STALLED) => record.unverified = true,
        Err(error) => return Err(error.message),
    }
    record.stage = Stage::Submitted;
    journal(record);
    Ok(())
}

/// How often and how long a resumed launch polls for readiness.
const READY_POLL: Duration = Duration::from_millis(750);
const READY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Ready,
    /// Another dialog is up: back to waiting.
    Blocked,
}

/// Waits until Herdr reports the agent idle and ready for input in two polls
/// in a row. Right after a startup dialog closes, an agent can look idle
/// while it is still starting (or updating itself), and keys typed then are
/// lost.
fn wait_until_ready(runner: &Runner, pane: &str, poll: Duration, timeout: Duration) -> Result<Readiness, String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut ready_polls = 0;
    loop {
        let agent = herdr(runner, &["agent", "get", pane], Duration::from_secs(15)).map_err(|e| e.message)?;
        let agent = agent.get("agent").unwrap_or(&agent);
        let status = agent.get("agent_status").and_then(Value::as_str).unwrap_or("unknown");
        let interactive = agent.get("interactive_ready").and_then(Value::as_bool).unwrap_or(false);
        match status {
            "blocked" => return Ok(Readiness::Blocked),
            "idle" | "done" if interactive => {
                ready_polls += 1;
                if ready_polls >= 2 {
                    return Ok(Readiness::Ready);
                }
            }
            _ => ready_polls = 0,
        }
        if std::time::Instant::now() >= deadline {
            return Err("the agent did not become ready for the task".into());
        }
        std::thread::sleep(poll);
    }
}

/// Sends the task of a launch that waited at a startup dialog, once the agent
/// is really ready. Another dialog sends it back to waiting.
pub fn resume(runner: &Runner, record: Record, journal: &dyn Fn(&Record)) -> Result<Outcome, Failure> {
    resume_with(runner, record, journal, READY_POLL, READY_TIMEOUT)
}

fn resume_with(
    runner: &Runner,
    mut record: Record,
    journal: &dyn Fn(&Record),
    poll: Duration,
    timeout: Duration,
) -> Result<Outcome, Failure> {
    let fail = |mut record: Record, error: String| {
        record.failed_stage = Some(record.stage);
        record.stage = Stage::NeedsAttention;
        record.error = Some(error.clone());
        journal(&record);
        Failure::new(record, error)
    };
    let Some(pane) = record.pane_id.clone() else {
        return Err(fail(record, "the launch has no pane".into()));
    };
    match wait_until_ready(runner, &pane, poll, timeout) {
        Ok(Readiness::Blocked) => return Ok(Outcome::WaitingForStartup(record)),
        Ok(Readiness::Ready) => {}
        Err(error) => return Err(fail(record, error)),
    }
    match submit(runner, &mut record, journal) {
        Ok(()) => Ok(Outcome::Sent(record)),
        Err(error) => Err(fail(record, error)),
    }
}

#[cfg(test)]
mod tests;
