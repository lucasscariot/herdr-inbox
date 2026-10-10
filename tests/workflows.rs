//! Runner-selection contracts for the trusted CI definition. These checks need
//! no GitHub token, YAML package or runner, and tolerate folded conditions.

const CI: &str = include_str!("../.github/workflows/ci.yml");
const RELEASE: &str = include_str!("../.github/workflows/release.yml");
const RELEVANT_EDIT: &str =
    "(github.event.action != 'edited' || github.event.changes.title || github.event.changes.base)";
const TRUSTED: &str = "(github.event_name != 'pull_request_target' || github.event.pull_request.head.repo.full_name == github.repository)";

fn condition(job: &str) -> String {
    condition_in(CI, job)
}

fn condition_in(workflow: &str, job: &str) -> String {
    let heading = format!("  {job}:");
    let mut lines = workflow.lines().skip_while(|line| *line != heading).skip(1);
    let first = lines.find(|line| line.starts_with("    if:")).expect("job condition");
    let value = first.trim().strip_prefix("if:").unwrap().trim();
    let value = if value == ">-" {
        lines.take_while(|line| line.starts_with("      ")).map(str::trim).collect::<Vec<_>>().join(" ")
    } else {
        value.to_string()
    };
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn description_only_edits_do_not_run_ci_or_overwrite_the_tested_commit_status() {
    assert_eq!(
        condition("title"),
        format!(
            "github.event_name == 'pull_request_target' && github.event.pull_request.head.repo.full_name == github.repository && {RELEVANT_EDIT}"
        )
    );
    for job in ["rust", "installer"] {
        assert_eq!(condition(job), format!("{TRUSTED} && {RELEVANT_EDIT}"), "{job}");
    }
    assert_eq!(condition("report"), format!("always() && !cancelled() && {TRUSTED} && {RELEVANT_EDIT}"));
}

#[test]
fn description_edits_cannot_cancel_or_replace_pending_code_checks() {
    assert!(CI.contains("github.event.action == 'edited' && !github.event.changes.title && !github.event.changes.base && 'description' || 'checks'"));
}

#[test]
fn the_release_workflow_does_not_repeat_ci_on_pr_heads() {
    let checks = condition_in(RELEASE, "checks");
    assert!(checks.contains("github.event_name != 'pull_request_target'"), "{checks}");
    let build = condition_in(RELEASE, "build");
    assert!(
        build.contains("(github.event_name == 'pull_request_target' || needs.checks.result == 'success')"),
        "{build}"
    );
    assert!(RELEASE.contains(r#"("$CHECKS_RESULT" == success || "$CHECKS_RESULT" == skipped)"#));
}

#[test]
fn the_release_pr_is_tested_by_ci_alone_and_obsolete_pr_builds_are_cancelled() {
    assert!(RELEASE.contains(r#"gh workflow run ci.yml --ref "$branch""#));
    assert!(
        !RELEASE.contains("gh workflow run release.yml"),
        "a release.yml dispatch builds four binaries on every main push"
    );
    assert!(RELEASE.contains("cancel-in-progress: ${{ github.event_name == 'pull_request_target' }}"));
}

#[test]
fn title_and_base_edits_still_have_a_ci_trigger() {
    assert!(CI.contains("types: [opened, synchronize, reopened, edited]"));
    assert!(CI.contains("cancel-in-progress: true"), "obsolete runs must not queue behind the new head");
}
