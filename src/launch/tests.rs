use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use std::time::Duration;

use super::*;
use crate::config::Config;
use crate::discovery::{CheckoutEntry, Choice};
use crate::herdr::terminal::HerdrCommand;

fn project() -> Project {
    Project {
        name: "cockpit".into(),
        path: "/w/cockpit".into(),
        branch: "main".into(),
        checkouts: vec![
            CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false },
            CheckoutEntry { path: "/h/wt/fix-login".into(), branch: "fix-login".into(), linked: true },
        ],
    }
}

fn request(task: &str, workspace: WorkspaceChoice) -> Request {
    Request {
        machine_id: "local".into(),
        machine_label: "Local".into(),
        project: project(),
        harness: "claude".into(),
        model: None,
        thinking: None,
        task: task.into(),
        workspace,
        comparison: None,
    }
}

fn settings() -> Effective {
    Config::default().for_machine("local", "Local", true)
}

fn claude() -> Catalog {
    Catalog {
        choices: vec![Choice { id: "opus".into(), label: "Opus".into() }],
        selectable: true,
        thinking_flag: "--effort".into(),
        thinking: vec!["high".into(), "max".into()],
        ..Catalog::default()
    }
}

fn plan_for(request: &Request, settings: &Effective, catalog: Option<&Catalog>) -> Result<Plan, String> {
    plan(request, settings, catalog, "0123456789abcdef".into(), UNIX_EPOCH)
}

#[test]
fn two_launches_in_the_same_second_get_different_agent_names() {
    let request = request("Fix login", WorkspaceChoice::NewWorktree { branch: None });
    let a = plan(&request, &settings(), None, "0000000100000001".into(), UNIX_EPOCH).unwrap();
    let b = plan(&request, &settings(), None, "0000000100000002".into(), UNIX_EPOCH).unwrap();
    assert_ne!(a.record.agent_name, b.record.agent_name);
    assert_eq!(short_id("short"), "short");
}

#[test]
fn compared_agents_get_short_telling_branch_suffixes() {
    assert_eq!(agent_slug("claude", None), "claude");
    assert_eq!(agent_slug("claude", Some("claude-opus-5-5")), "claude-opus-5-5", "the harness is not repeated");
    assert_eq!(agent_slug("codex", Some("gpt-5")), "codex-gpt-5");
    assert_eq!(agent_slug("pi", Some("openai-codex/gpt-6.1-sol")), "pi-openai-codex-gpt-6-1-sol");
    assert_eq!(agent_slug("pi", Some(&"m".repeat(40))).len(), "pi-".len() + 24, "long ids are cut");
    assert_eq!(agent_slug("Claude Code", Some("  ")), "claude-code", "a blank model is no model");
    assert_eq!(agent_slug("", None), "agent");
    assert_eq!(unique_branch("fix", &["fix", "fix-2"]), "fix-3");
    assert_eq!(unique_branch("fix", &[]), "fix");
}

#[test]
fn a_plan_keeps_the_agents_place_in_a_comparison() {
    let mut compared = request("Fix login", WorkspaceChoice::NewWorktree { branch: Some("fix-login-claude".into()) });
    compared.comparison = Some(Comparison { index: 1, total: 3 });
    let plan = plan_for(&compared, &settings(), Some(&claude())).unwrap();
    assert_eq!(plan.record.comparison, Some(Comparison { index: 1, total: 3 }));
    assert_eq!(plan.record.branch, "fix-login-claude");
    let json = serde_json::to_string(&plan.record).unwrap();
    assert!(json.contains("\"comparison\":{\"index\":1,\"total\":3}"), "{json}");
    let single =
        plan_for(&request("Fix login", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    assert!(!serde_json::to_string(&single.record).unwrap().contains("comparison"), "absent for a single agent");
    let old: Record = serde_json::from_str(&serde_json::to_string(&single.record).unwrap()).unwrap();
    assert_eq!(old.comparison, None, "journals from before comparisons still read");
}

#[test]
fn titles_are_one_line_and_bounded() {
    assert_eq!(task_title("  Fix the\n  login   loop  "), "Fix the login loop");
    assert_eq!(task_title(&"x".repeat(100)).len(), 72);
    assert_eq!(task_title("é".repeat(80).as_str()).chars().count(), 72, "counts characters, not bytes");
}

#[test]
fn branch_names_keep_the_content_words_of_the_first_sentence() {
    assert_eq!(branch_name("Fix the login redirect loop on mobile", &[], ""), "fix-login-redirect-loop");
    assert_eq!(branch_name("Please can you add a CSV export?", &[], ""), "add-csv-export");
    assert_eq!(branch_name("Refactor\nsecond line ignored", &[], ""), "refactor");
    assert_eq!(
        branch_name("The worktree naming we generate is bad - it doesn't capture the essence of the request", &[], ""),
        "worktree-naming-generate-bad",
        "the first sentence names the task; the dash starts another"
    );
    assert_eq!(branch_name("Why does the sidebar flicker on resize?", &[], ""), "sidebar-flicker-resize");
    assert_eq!(
        branch_name("Can we make the composer remember the last model?", &[], ""),
        "composer-remember-last-model"
    );
    assert_eq!(branch_name("I'd like to be able to archive threads with `x`.", &[], ""), "archive-threads-x");
    assert_eq!(branch_name("It doesn't work when I paste an image", &[], ""), "not-work-paste-image");
    assert_eq!(
        branch_name("Go through codex, opencode, claude - check how they handle this.", &[], ""),
        "codex-opencode-claude"
    );
}

#[test]
fn branch_names_keep_negations() {
    assert_eq!(branch_name("Do not delete backups", &[], ""), "not-delete-backups");
    assert_eq!(branch_name("Don't delete backups", &[], ""), "not-delete-backups", "contractions count");
    assert_eq!(branch_name("Never retry a failed launch", &[], ""), "not-retry-failed-launch");
    assert_eq!(branch_name("No auth required", &[], ""), "no-auth-required");
}

#[test]
fn branch_names_keep_the_direction_of_a_request() {
    assert_eq!(branch_name("Turn on authentication", &[], ""), "turn-on-authentication");
    assert_eq!(branch_name("Turn off authentication", &[], ""), "turn-off-authentication");
    assert_eq!(branch_name("Please turn authentication off", &[], ""), "turn-authentication-off");
    assert_eq!(branch_name("Use more logging", &[], ""), "use-more-logging");
    assert_eq!(branch_name("Use less logging", &[], ""), "use-less-logging");
    assert_eq!(branch_name("Clean up the tests", &[], ""), "clean-up-tests", "a particle completes the leading verb");
    assert_eq!(branch_name("Sign in button is broken", &[], ""), "sign-in-button-broken");
    assert_eq!(
        branch_name("Fix the login loop on mobile", &[], ""),
        "fix-login-loop-mobile",
        "but is filler as a preposition"
    );
}

#[test]
fn branch_names_skip_sentences_that_say_nothing() {
    assert_eq!(branch_name("Hi there! Fix the login loop.", &[], ""), "fix-login-loop", "greetings are skipped");
    assert_eq!(branch_name("Hi there!", &[], ""), "hi-there", "pure filler still names the task when nothing follows");
    assert_eq!(branch_name("Please\n\nFix the login loop", &[], ""), "fix-login-loop", "and so are filler lines");
    assert_eq!(branch_name("!!!", &[], ""), "thread");
    assert_eq!(branch_name("Übersetze die Seite", &[], ""), "bersetze-die-seite");
}

#[test]
fn branch_names_stay_short() {
    let long = branch_name("implement the incredibly comprehensive authentication middleware rewrite", &[], "");
    assert!(long.len() <= 32, "{long}");
    assert_eq!(long, "implement-incredibly", "four words at most, cut at a word boundary to fit 32 characters");
    assert_eq!(branch_name(&"x".repeat(50), &[], "").len(), 32, "one huge word is clipped");
    assert_eq!(
        branch_name("Bump v1.2 for the release", &[], ""),
        "bump-v1-2-release",
        "dots inside words do not end a sentence"
    );
}

#[test]
fn branch_names_take_a_prefix_and_avoid_taken_ones() {
    assert_eq!(branch_name("fix login", &[], "lucas/"), "lucas/fix-login");
    assert_eq!(branch_name("fix login", &["fix-login"], ""), "fix-login-2");
    assert_eq!(branch_name("fix login", &["fix-login", "fix-login-2"], ""), "fix-login-3");
}

#[test]
fn branch_validation_follows_git() {
    for good in ["fix", "feat/login", "v1.2", "a_b-c"] {
        assert!(validate_branch(good).is_ok(), "{good}");
    }
    for bad in ["", "-x", "/x", "x/", "a..b", "a//b", "a@{b", "x.lock", "a b", "é"] {
        assert!(validate_branch(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_new_worktree_gets_a_branch_from_the_task() {
    let plan =
        plan_for(&request("Fix login loop", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    assert_eq!(plan.record.workspace, "worktree");
    assert_eq!(plan.record.branch, "fix-login-loop");
    assert_eq!(plan.record.cwd, "");
    assert_eq!(plan.record.stage, Stage::Creating);
    assert_eq!(plan.record.agent_name, "t-fix-login-loop-4567cdef");
    assert_eq!(plan.record.title, "Fix login loop");
}

#[test]
fn a_new_worktree_never_reuses_a_branch_that_has_one() {
    let derived =
        plan_for(&request("fix login", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    assert_eq!(derived.record.branch, "fix-login-2", "the derived name steps aside");
    let named = request("x", WorkspaceChoice::NewWorktree { branch: Some("fix-login".into()) });
    assert!(plan_for(&named, &settings(), None).unwrap_err().contains("already has a worktree"));
    let invalid = request("x", WorkspaceChoice::NewWorktree { branch: Some("bad name".into()) });
    assert!(plan_for(&invalid, &settings(), None).is_err());
}

#[test]
fn a_checkout_runs_in_place_and_falls_back_to_the_main_one() {
    let linked =
        plan_for(&request("x", WorkspaceChoice::Checkout { path: "/h/wt/fix-login".into() }), &settings(), None)
            .unwrap();
    assert_eq!((linked.record.cwd.as_str(), linked.record.branch.as_str()), ("/h/wt/fix-login", "fix-login"));
    let gone = plan_for(&request("x", WorkspaceChoice::Checkout { path: "/gone".into() }), &settings(), None).unwrap();
    assert_eq!(gone.record.cwd, "/w/cockpit");
}

#[test]
fn an_empty_task_is_refused() {
    assert!(plan_for(&request("   \n ", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).is_err());
}

#[test]
fn model_and_thinking_replace_pinned_flags_in_harness_args() {
    let mut settings = settings();
    settings.harness_args.insert(
        "claude".into(),
        vec!["--verbose".into(), "--model".into(), "sonnet".into(), "--effort=low".into(), "-m".into(), "x".into()],
    );
    let mut request = request("x", WorkspaceChoice::Checkout { path: "/w/cockpit".into() });
    request.model = Some("opus".into());
    request.thinking = Some("max".into());
    let plan = plan_for(&request, &settings, Some(&claude())).unwrap();
    assert_eq!(plan.agent_args, ["--verbose", "--model", "opus", "--effort", "max"]);
    assert_eq!(plan.record.model, "opus");
    assert_eq!(plan.record.thinking, "max");
}

#[test]
fn codex_effort_overrides_only_the_reasoning_config_and_validates_the_model() {
    let catalog: Catalog = serde_json::from_value(serde_json::json!({
        "selectable": true, "thinking_flag": "--config", "thinking": ["low", "high", "ultra"],
        "default": "gpt-deep", "thinking_by_model": {"gpt-deep": ["low", "high", "ultra"], "gpt-lite": ["low"]}
    }))
    .unwrap();
    let mut settings = settings();
    settings.harness_args.insert(
        "codex".into(),
        [
            "--no-daemon",
            "-c",
            "model_reasoning_effort=\"low\"",
            "--config=model_reasoning_effort='high'",
            "-cmodel_reasoning_effort=low",
            "-c=model_reasoning_effort=low",
            "--config",
            "model_reasoning_effort = 'low'",
            "-c",
            "sandbox_mode=\"read-only\"",
            "--config=features.foo=true",
        ]
        .map(String::from)
        .to_vec(),
    );
    let mut request = request("x", WorkspaceChoice::Checkout { path: "/w/cockpit".into() });
    request.harness = "codex".into();
    request.thinking = Some("ultra".into());
    let plan = plan_for(&request, &settings, Some(&catalog)).unwrap();
    assert_eq!(
        plan.agent_args,
        [
            "--no-daemon",
            "-c",
            "sandbox_mode=\"read-only\"",
            "--config=features.foo=true",
            "--config",
            "model_reasoning_effort=\"ultra\""
        ]
    );
    request.model = Some("gpt-lite".into());
    assert!(plan_for(&request, &settings, Some(&catalog)).unwrap_err().contains("thinking level ultra"));
    request.thinking = Some("low".into());
    assert!(plan_for(&request, &settings, Some(&catalog)).is_ok());
    request.thinking = None;
    assert!(
        plan_for(&request, &settings, Some(&catalog))
            .unwrap()
            .agent_args
            .contains(&"model_reasoning_effort=\"low\"".to_string()),
        "default leaves CLI configuration alone"
    );
    request.harness = "claude".into();
    request.thinking = Some("low".into());
    assert!(plan_for(&request, &settings, Some(&catalog)).is_err(), "config overrides are specific to Codex");
}

#[test]
fn harness_args_pass_through_untouched_without_choices() {
    let mut settings = settings();
    settings.harness_args.insert("claude".into(), vec!["--model".into(), "sonnet".into()]);
    let plan =
        plan_for(&request("x", WorkspaceChoice::Checkout { path: "/w/cockpit".into() }), &settings, Some(&claude()))
            .unwrap();
    assert_eq!(plan.agent_args, ["--model", "sonnet"]);
}

#[test]
fn unsupported_models_and_thinking_levels_are_refused_before_anything_runs() {
    let mut request = request("x", WorkspaceChoice::Checkout { path: "/w/cockpit".into() });
    request.model = Some("opus".into());
    let fixed = Catalog { selectable: false, ..claude() };
    assert!(plan_for(&request, &settings(), Some(&fixed)).unwrap_err().contains("model"));
    request.model = None;
    request.thinking = Some("ultra".into());
    assert!(plan_for(&request, &settings(), Some(&claude())).unwrap_err().contains("thinking level ultra"));
    request.thinking = Some("high".into());
    let no_flag = Catalog { thinking_flag: "--mystery".into(), ..claude() };
    assert!(plan_for(&request, &settings(), Some(&no_flag)).is_err());
    request.thinking = Some("  ".into());
    assert!(plan_for(&request, &settings(), None).is_ok(), "blank means default");
}

/// A fake `herdr` that logs every call and answers like Herdr would.
/// `AGENT_START` and `PROMPT` pick the outcome of those steps.
fn fake_herdr(dir: &Path, agent_start: &str, prompt: &str) -> Runner {
    let log = dir.join("calls.log");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
case "$1 $2" in
  "worktree create") echo '{{"id":"c","result":{{"type":"worktree_created","workspace":{{"workspace_id":"w5","worktree":{{"checkout_path":"/h/wt/new"}}}},"root_pane":{{"pane_id":"w5:p1","cwd":"/h/wt/new"}},"tab":{{"tab_id":"w5:t1"}}}}}}' ;;
  "workspace create") echo '{{"id":"c","result":{{"type":"workspace_created","workspace":{{"workspace_id":"w6"}},"root_pane":{{"pane_id":"w6:p1"}},"tab":{{"tab_id":"w6:t1"}}}}}}' ;;
  "agent start") case '{agent_start}' in
      ok) echo '{{"id":"c","result":{{"type":"agent_started"}}}}' ;;
      blocked) echo '{{"id":"c","error":{{"code":"agent_not_ready","message":"agent t-x is blocked during startup and is not ready for prompts"}}}}'; exit 1 ;;
      broken) echo '{{"id":"c","error":{{"code":"agent_kind_mismatch","message":"expected claude, detected bash"}}}}' >&2; exit 1 ;;
    esac ;;
  "agent get") n=$(cat '{counter}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{counter}'
      line=$(sed -n "${{n}}p" '{readiness}'); [ -z "$line" ] && line=$(tail -n 1 '{readiness}')
      echo "$line" ;;
  "agent prompt") case '{prompt}' in
      ok) echo '{{"id":"c","result":{{"type":"agent_prompted"}}}}' ;;
      stalled) echo '{{"id":"c","error":{{"code":"agent_prompt_stalled","message":"no observed working or blocked state"}}}}'; exit 1 ;;
      blocked) echo '{{"id":"c","error":{{"code":"agent_blocked","message":"agent is blocked"}}}}'; exit 1 ;;
    esac ;;
  *) echo '{{"id":"c","result":{{"type":"ok"}}}}' ;;
esac
"#,
        log = log.display(),
        counter = dir.join("get.count").display(),
        readiness = dir.join("readiness").display(),
    );
    // Without a scripted readiness, the agent is ready at once.
    if !dir.join("readiness").exists() {
        std::fs::write(dir.join("readiness"), ready(true, "idle")).unwrap();
    }
    let path = dir.join("herdr");
    crate::testing::write_executable(&path, &script);
    Runner::Local(HerdrCommand::new(path))
}

/// One `herdr agent get` answer.
fn ready(interactive: bool, status: &str) -> String {
    format!(
        "{{\"id\":\"c\",\"result\":{{\"type\":\"agent_info\",\"agent\":{{\"agent_status\":\"{status}\",\"interactive_ready\":{interactive}}}}}}}\n"
    )
}

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

type Logged = (Result<Outcome, Failure>, Vec<Stage>, Vec<String>);

fn execute_logged(runner: &Runner, plan: Plan) -> Logged {
    let stages = Mutex::new(Vec::new());
    let progress = Mutex::new(Vec::new());
    let result = execute(runner, plan, &|r| stages.lock().unwrap().push(r.stage), &|p| {
        progress.lock().unwrap().push(p.to_string())
    });
    (result, stages.into_inner().unwrap(), progress.into_inner().unwrap())
}

#[test]
fn a_worktree_launch_runs_every_step_in_order_and_journals_each() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "ok");
    let mut request = request("Fix login loop", WorkspaceChoice::NewWorktree { branch: None });
    request.thinking = Some("high".into());
    let plan = plan_for(&request, &settings(), Some(&claude())).unwrap();
    let (result, stages, progress) = execute_logged(&runner, plan);
    let Ok(Outcome::Sent(record)) = result else { panic!("{result:?}") };
    assert_eq!(record.stage, Stage::Submitted);
    assert_eq!(record.workspace_id.as_deref(), Some("w5"));
    assert_eq!(record.pane_id.as_deref(), Some("w5:p1"));
    assert_eq!(record.cwd, "/h/wt/new");
    assert!(!record.unverified);
    assert_eq!(
        stages,
        [Stage::Creating, Stage::Created, Stage::Starting, Stage::Ready, Stage::Submitting, Stage::Submitted]
    );
    assert_eq!(progress, ["creating worktree fix-login-loop", "starting claude", "sending the task"]);
    assert_eq!(
        calls(dir.path()),
        [
            "worktree create --cwd /w/cockpit --branch fix-login-loop --label cockpit --no-focus",
            "tab rename w5:t1 Fix login loop",
            "agent start t-fix-login-loop-4567cdef --kind claude --pane w5:p1 --timeout 45000 -- --effort high",
            "pane report-metadata w5:p1 --source herdr-inbox --display-agent Claude --token thread=Fix login loop",
            "agent prompt w5:p1 Fix login loop --wait --until working --until blocked --timeout 15000",
        ]
    );
}

#[test]
fn a_checkout_launch_creates_a_plain_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "ok");
    let plan =
        plan_for(&request("x", WorkspaceChoice::Checkout { path: "/w/cockpit".into() }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(&runner, plan);
    assert!(matches!(result, Ok(Outcome::Sent(_))));
    assert_eq!(calls(dir.path())[0], "workspace create --cwd /w/cockpit --label cockpit --no-focus");
    assert!(calls(dir.path())[2].ends_with("--timeout 45000"), "no `--` without agent args");
}

#[test]
fn a_startup_dialog_keeps_the_task_for_later() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "blocked", "ok");
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, stages, _) = execute_logged(&runner, plan);
    let Ok(Outcome::WaitingForStartup(record)) = result else { panic!("{result:?}") };
    assert_eq!(record.stage, Stage::StartupBlocked);
    assert_eq!(stages.last(), Some(&Stage::StartupBlocked));
    let calls = calls(dir.path());
    assert!(calls.iter().any(|c| c.starts_with("pane report-metadata")), "the thread is still labelled");
    assert!(!calls.iter().any(|c| c.starts_with("agent prompt")), "the task waits");

    let Ok(Outcome::Sent(resumed)) = resume_with(&runner, record, &|_| {}, Duration::ZERO, Duration::from_secs(5))
    else {
        panic!("not sent");
    };
    assert_eq!(resumed.stage, Stage::Submitted);
    assert!(super::tests::calls(dir.path()).last().unwrap().starts_with("agent prompt w5:p1 x"));
}

fn waiting_record(runner: &Runner) -> Record {
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(runner, plan);
    match result {
        Ok(Outcome::WaitingForStartup(record)) => record,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_resume_waits_until_the_agent_is_ready_twice_in_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let script =
        [ready(false, "idle"), ready(true, "idle"), ready(false, "working"), ready(true, "done"), ready(true, "idle")]
            .concat();
    std::fs::write(dir.path().join("readiness"), script).unwrap();
    let runner = fake_herdr(dir.path(), "blocked", "ok");
    let record = waiting_record(&runner);
    let result = resume_with(&runner, record, &|_| {}, Duration::ZERO, Duration::from_secs(5));
    assert!(matches!(result, Ok(Outcome::Sent(_))), "{result:?}");
    let gets = calls(dir.path()).iter().filter(|c| c.starts_with("agent get")).count();
    assert_eq!(gets, 5, "not ready, ready, busy again, then ready twice");
}

#[test]
fn a_second_dialog_sends_the_launch_back_to_waiting_without_typing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readiness"), ready(true, "blocked")).unwrap();
    let runner = fake_herdr(dir.path(), "blocked", "ok");
    let record = waiting_record(&runner);
    let result = resume_with(&runner, record, &|_| {}, Duration::ZERO, Duration::from_secs(5));
    assert!(matches!(result, Ok(Outcome::WaitingForStartup(_))), "{result:?}");
    assert!(!calls(dir.path()).iter().any(|c| c.starts_with("agent prompt")));
}

#[test]
fn an_agent_that_never_gets_ready_fails_the_resume() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readiness"), ready(false, "idle")).unwrap();
    let runner = fake_herdr(dir.path(), "blocked", "ok");
    let record = waiting_record(&runner);
    let Err(failure) = resume_with(&runner, record, &|_| {}, Duration::ZERO, Duration::from_millis(50)) else {
        panic!("should fail");
    };
    assert_eq!(failure.error, "the agent did not become ready for the task");
    assert_eq!(failure.record.stage, Stage::NeedsAttention);
}

#[test]
fn a_stalled_prompt_is_sent_but_unverified_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "stalled");
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(&runner, plan);
    let Ok(Outcome::Sent(record)) = result else { panic!("{result:?}") };
    assert!(record.unverified);
    assert_eq!(record.stage, Stage::Submitted);
    assert_eq!(calls(dir.path()).iter().filter(|c| c.starts_with("agent prompt")).count(), 1);
}

#[test]
fn a_failed_step_records_where_it_stopped_and_why() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "broken", "ok");
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, stages, _) = execute_logged(&runner, plan);
    let Err(Failure { record, error }) = result else { panic!("{result:?}") };
    assert_eq!(error, "expected claude, detected bash");
    assert_eq!(record.stage, Stage::NeedsAttention);
    assert_eq!(record.failed_stage, Some(Stage::Starting));
    assert_eq!(record.pane_id.as_deref(), Some("w5:p1"), "the pane is known, so the user can inspect it");
    assert_eq!(stages.last(), Some(&Stage::NeedsAttention));
}

#[test]
fn a_blocked_agent_at_prompt_time_is_a_failure_not_a_replay() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "blocked");
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(&runner, plan);
    let Err(Failure { record, .. }) = result else { panic!("{result:?}") };
    assert_eq!(record.failed_stage, Some(Stage::Submitting));
}

#[test]
fn plain_stderr_is_the_last_resort_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr");
    crate::testing::write_executable(
        &path,
        "#!/bin/sh\necho 'warning: noise' >&2\necho 'error: no such session' >&2\nexit 2\n",
    );
    let err = herdr(&Runner::Local(HerdrCommand::new(path)), &["ping"], Duration::from_secs(5)).unwrap_err();
    assert_eq!(err, CliError { code: None, message: "error: no such session".into() });
}

#[test]
fn a_silent_success_is_an_empty_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr");
    crate::testing::write_executable(&path, "#!/bin/sh\nexit 0\n");
    assert_eq!(herdr(&Runner::Local(HerdrCommand::new(path)), &["x"], Duration::from_secs(5)).unwrap(), Value::Null);
}

#[test]
fn a_hung_herdr_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr");
    crate::testing::write_executable(&path, "#!/bin/sh\nsleep 30\n");
    let started = std::time::Instant::now();
    let err = herdr(&Runner::Local(HerdrCommand::new(path)), &["x"], Duration::from_millis(300)).unwrap_err();
    assert_eq!(err.message, "herdr did not answer in time");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_herdr_fails_at_creation() {
    let runner = Runner::Local(HerdrCommand::new("/nonexistent/herdr"));
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(&runner, plan);
    let Err(Failure { record, error }) = result else { panic!("{result:?}") };
    assert_eq!(record.failed_stage, Some(Stage::Creating));
    assert!(error.contains("cannot run herdr"), "{error}");
}

#[test]
fn records_round_trip_through_json() {
    let plan = plan_for(&request("x", WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let json = serde_json::to_string(&plan.record).unwrap();
    assert!(json.contains("\"stage\":\"creating\""));
    let back: Record = serde_json::from_str(&json).unwrap();
    assert_eq!(back, plan.record);
}

const IMAGE_A: &str = "/home/u/.cache/herdr-inbox/images/image-1.png";
const IMAGE_B: &str = "/home/u/.cache/herdr-inbox/images/image-2.png";

#[test]
fn pasted_images_are_left_out_of_the_title_and_branch() {
    let task = format!("{IMAGE_A}\nwhy is the login {IMAGE_B} broken");
    let plan = plan_for(&request(&task, WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    assert_eq!(plan.record.title, "why is the login broken");
    assert_eq!(plan.record.branch, "login-broken");
    assert_eq!(plan.record.task, task, "the task keeps its images");
    let only = plan_for(&request(IMAGE_A, WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    assert_eq!((only.record.title.as_str(), only.record.branch.as_str()), ("image", "image"));
}

#[test]
fn each_image_is_pasted_on_its_own_then_the_rest_is_prompted() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "ok");
    let task = format!("why {IMAGE_A}\nbroken {IMAGE_B} see?");
    let plan = plan_for(&request(&task, WorkspaceChoice::NewWorktree { branch: None }), &settings(), None).unwrap();
    let (result, _, _) = execute_logged(&runner, plan);
    assert!(matches!(result, Ok(Outcome::Sent(_))), "{result:?}");
    let calls = calls(dir.path());
    let sent: Vec<&str> = calls.iter().skip_while(|c| !c.starts_with("pane send-text")).map(String::as_str).collect();
    assert_eq!(
        sent,
        [
            "pane send-text w5:p1 \x1b[200~why \x1b[201~".to_string(),
            format!("pane send-text w5:p1 \x1b[200~{IMAGE_A}\x1b[201~"),
            "pane send-text w5:p1 \x1b[200~\rbroken \x1b[201~".to_string(),
            format!("pane send-text w5:p1 \x1b[200~{IMAGE_B}\x1b[201~"),
            "agent prompt w5:p1  see? --wait --until working --until blocked --timeout 15000".to_string(),
        ]
    );
}

#[test]
fn a_task_ending_with_an_image_prompts_with_that_image() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "ok");
    assert_eq!(prompt(&runner, "w2:p1", &format!("look: {IMAGE_A}")), Ok(false));
    assert_eq!(
        calls(dir.path()),
        [
            "pane send-text w2:p1 \x1b[200~look: \x1b[201~".to_string(),
            format!("agent prompt w2:p1 {IMAGE_A} --wait --until working --until blocked --timeout 8000"),
        ]
    );
}

#[test]
fn text_without_saved_images_is_prompted_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let runner = fake_herdr(dir.path(), "ok", "ok");
    assert_eq!(prompt(&runner, "w2:p1", "read /tmp/shot.png"), Ok(false));
    assert_eq!(
        calls(dir.path()),
        ["agent prompt w2:p1 read /tmp/shot.png --wait --until working --until blocked --timeout 8000"]
    );
}
