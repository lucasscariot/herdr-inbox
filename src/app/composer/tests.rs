use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::discovery::{CheckoutEntry, Choice as ModelChoice};
use crate::launch::Comparison;
use crate::state::{ProjectChoices, Remembered};

fn machine(id: &str, label: &str, connection: Connection) -> MachineState {
    MachineState { id: id.into(), label: label.into(), connection, snapshot: None, checkouts: HashMap::new() }
}

fn project(name: &str, branches: &[(&str, bool)]) -> Project {
    let path = format!("/w/{name}");
    Project {
        name: name.into(),
        path: path.clone(),
        branch: "main".into(),
        checkouts: branches
            .iter()
            .map(|(branch, linked)| CheckoutEntry {
                path: if *linked { format!("/h/wt/{branch}") } else { path.clone() },
                branch: branch.to_string(),
                linked: *linked,
            })
            .collect(),
    }
}

fn claude() -> Catalog {
    Catalog {
        choices: vec![
            ModelChoice { id: "opus".into(), label: "Opus".into() },
            ModelChoice { id: "sonnet".into(), label: "Sonnet".into() },
        ],
        selectable: true,
        default: "opus".into(),
        thinking_flag: "--effort".into(),
        thinking: vec!["high".into(), "max".into()],
        ..Catalog::default()
    }
}

fn codex() -> Catalog {
    Catalog {
        choices: vec![ModelChoice { id: "gpt-5".into(), label: "GPT-5".into() }],
        selectable: true,
        ..Catalog::default()
    }
}

struct World {
    machines: Vec<MachineState>,
    inventories: HashMap<String, Inventory>,
    preferences: Preferences,
    config: Config,
    presets: Vec<Preset>,
    usage: HashMap<String, crate::usage::Report>,
}

impl World {
    fn ctx(&self) -> Context<'_> {
        Context {
            machines: &self.machines,
            inventories: &self.inventories,
            preferences: &self.preferences,
            config: &self.config,
            presets: &self.presets,
            usage: &self.usage,
            now: std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000),
        }
    }
}

/// Local has cockpit and api with claude and codex; the studio has api with
/// codex only; the book is unreachable but known to have cockpit.
fn world() -> World {
    let mut inventories = HashMap::new();
    inventories.insert(
        "local".to_string(),
        Inventory {
            projects: vec![
                project("cockpit", &[("main", false), ("fix-login", true)]),
                project("api", &[("main", false)]),
            ],
            harnesses: vec!["codex".into(), "claude".into()],
            models: BTreeMap::from([("claude".into(), claude()), ("codex".into(), codex())]),
            models_at: 1,
            models_revision: crate::discovery::MODELS_REVISION,
        },
    );
    inventories.insert(
        "studio".to_string(),
        Inventory {
            projects: vec![project("api", &[("main", false)])],
            harnesses: vec!["codex".into()],
            models: BTreeMap::from([("codex".into(), codex())]),
            models_at: 1,
            models_revision: crate::discovery::MODELS_REVISION,
        },
    );
    inventories.insert(
        "book".to_string(),
        Inventory {
            projects: vec![project("cockpit", &[("main", false)])],
            harnesses: vec!["claude".into()],
            ..Inventory::default()
        },
    );
    World {
        machines: vec![
            machine("local", "Local", Connection::Live),
            machine("studio", "Mac Studio", Connection::Live),
            machine("book", "MacBook", Connection::Lost("timed out".into())),
        ],
        inventories,
        preferences: Preferences::default(),
        config: Config::default(),
        presets: Vec::new(),
        usage: HashMap::new(),
    }
}

fn settled(world: &World) -> Composer {
    let mut composer = Composer::default();
    composer.settle(&world.ctx());
    composer
}

fn remember(
    world: &mut World,
    project: &str,
    machine: &str,
    harness: &str,
    model: &str,
    thinking: &str,
    workspace: &str,
) {
    world.preferences.remember(&Remembered {
        auto: false,
        project: project.into(),
        machine: machine.into(),
        harness: harness.into(),
        workspace: workspace.into(),
        model: model.into(),
        thinking: thinking.into(),
    });
}

#[test]
fn without_history_the_first_project_and_a_preferred_harness_are_chosen() {
    let world = world();
    let composer = settled(&world);
    assert_eq!(composer.project.as_deref(), Some("api"), "alphabetical first");
    assert_eq!(composer.machine.as_deref(), Some("local"), "the first live machine with it");
    assert_eq!(composer.harness.as_deref(), Some("claude"), "claude is preferred over codex");
    assert_eq!(composer.model, None);
    assert_eq!(composer.workspace, WorkspaceSel::New, "worktree-first by default");
}

#[test]
fn the_last_launch_is_remembered_per_project_machine_and_harness() {
    let mut world = world();
    remember(&mut world, "api", "studio", "codex", "gpt-5", "", "worktree");
    remember(&mut world, "cockpit", "local", "claude", "sonnet", "max", "checkout");
    let composer = settled(&world);
    assert_eq!(composer.project.as_deref(), Some("cockpit"), "the last project opens first");
    assert_eq!(composer.harness.as_deref(), Some("claude"));
    assert_eq!(composer.model.as_deref(), Some("sonnet"));
    assert_eq!(composer.thinking.as_deref(), Some("max"));
    let mut composer = composer;
    composer.apply(&world.ctx(), Pick::Project("api".into()));
    assert_eq!(composer.machine.as_deref(), Some("studio"), "api remembers the studio");
    assert_eq!(composer.harness.as_deref(), Some("codex"));
    assert_eq!(composer.model.as_deref(), Some("gpt-5"));
    assert_eq!(composer.thinking, None, "thinking is per harness too");
}

#[test]
fn switching_machine_never_carries_another_hosts_model() {
    let mut world = world();
    remember(&mut world, "api", "local", "codex", "gpt-5", "", "worktree");
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Harness("codex".into()));
    assert_eq!(composer.model.as_deref(), Some("gpt-5"));
    composer.apply(&world.ctx(), Pick::Machine("studio".into()));
    assert_eq!(composer.harness.as_deref(), Some("codex"), "the only harness there");
    assert_eq!(composer.model, None, "the studio has no remembered model");
}

#[test]
fn a_remembered_choice_that_is_no_longer_valid_falls_back() {
    let mut world = world();
    remember(&mut world, "api", "local", "claude", "opus", "ultra", "worktree");
    world.inventories.get_mut("local").unwrap().harnesses.retain(|h| h != "claude");
    let composer = settled(&world);
    assert_eq!(composer.harness.as_deref(), Some("codex"), "claude is gone from this machine");
    let mut world = self::world();
    remember(&mut world, "api", "local", "claude", "opus", "ultra", "worktree");
    let composer = settled(&world);
    assert_eq!(composer.thinking, None, "ultra is not a level claude offers");
    assert_eq!(composer.model.as_deref(), Some("opus"));
}

#[test]
fn a_model_is_dropped_when_the_cli_cannot_choose_one() {
    let mut world = world();
    remember(&mut world, "api", "local", "claude", "opus", "", "worktree");
    world.inventories.get_mut("local").unwrap().models.get_mut("claude").unwrap().selectable = false;
    assert_eq!(settled(&world).model, None);
}

#[test]
fn only_machines_that_have_the_project_are_offered_and_unreachable_ones_are_disabled() {
    let world = world();
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    let choices = composer.choices(&world.ctx(), Field::Machine, "");
    let labels: Vec<(&str, bool)> = choices.iter().map(|c| (c.label.as_str(), c.enabled)).collect();
    assert_eq!(labels, [("Local", true), ("MacBook", false)]);
    assert_eq!(choices[1].detail, "unreachable");
}

#[test]
fn a_project_on_an_unreachable_machine_only_still_settles_there() {
    let mut world = world();
    world.inventories.get_mut("local").unwrap().projects.retain(|p| p.name != "cockpit");
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    assert_eq!(composer.machine.as_deref(), Some("book"));
    let err = composer.requests(&world.ctx()).unwrap_err();
    assert_eq!(err, "Write a task first.");
    composer.task.set("Fix it");
    assert_eq!(composer.requests(&world.ctx()).unwrap_err(), "MacBook is not reachable right now.");
}

#[test]
fn project_names_are_merged_across_machines_with_the_last_one_first() {
    let mut world = world();
    assert_eq!(world.ctx().project_names(), ["api", "cockpit"]);
    world.preferences.last_project = Some("cockpit".into());
    assert_eq!(world.ctx().project_names(), ["cockpit", "api"]);
    world.preferences.last_project = Some("deleted".into());
    assert_eq!(world.ctx().project_names(), ["api", "cockpit"]);
    let choices = settled(&world).choices(&world.ctx(), Field::Project, "");
    assert_eq!(choices[0].detail, "Local, Mac Studio");
}

#[test]
fn the_thinking_field_shows_only_for_harnesses_with_levels() {
    let world = world();
    let mut composer = settled(&world);
    assert!(composer.fields(&world.ctx()).contains(&Field::Thinking));
    composer.apply(&world.ctx(), Pick::Harness("codex".into()));
    assert!(!composer.fields(&world.ctx()).contains(&Field::Thinking));
    composer.field = Field::Harness;
    composer.next_field(&world.ctx(), true);
    assert_eq!(composer.field, Field::Model);
    composer.next_field(&world.ctx(), true);
    assert_eq!(composer.field, Field::Workspace, "thinking is skipped");
    composer.next_field(&world.ctx(), true);
    assert_eq!(composer.field, Field::Compare);
    composer.next_field(&world.ctx(), true);
    assert_eq!(composer.field, Field::Task, "wraps around");
    composer.next_field(&world.ctx(), false);
    assert_eq!(composer.field, Field::Compare);
}

#[test]
fn thinking_choices_follow_the_model_and_drop_an_incompatible_level() {
    let mut world = world();
    let catalog = world.inventories.get_mut("local").unwrap().models.get_mut("codex").unwrap();
    catalog.default = "gpt-5".into();
    catalog.thinking_flag = "--config".into();
    catalog.thinking = vec!["low".into(), "high".into(), "ultra".into()];
    catalog.thinking_by_model = BTreeMap::from([
        ("gpt-5".into(), vec!["low".into(), "high".into(), "ultra".into()]),
        ("gpt-lite".into(), vec!["low".into()]),
        ("no-reasoning".into(), vec![]),
    ]);
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Harness("codex".into()));
    assert!(composer.fields(&world.ctx()).contains(&Field::Thinking));
    composer.apply(&world.ctx(), Pick::Thinking(Some("ultra".into())));
    assert_eq!(composer.choices(&world.ctx(), Field::Thinking, "").len(), 4);
    composer.apply(&world.ctx(), Pick::Model(Some("gpt-lite".into())));
    assert_eq!(composer.thinking, None, "ultra must not leak to a model that cannot use it");
    let levels: Vec<_> = composer.choices(&world.ctx(), Field::Thinking, "").into_iter().map(|c| c.label).collect();
    assert_eq!(levels, ["Default thinking", "low"]);
    composer.apply(&world.ctx(), Pick::Thinking(Some("low".into())));
    composer.apply(&world.ctx(), Pick::Model(Some("gpt-5".into())));
    assert_eq!(composer.thinking.as_deref(), Some("low"), "compatible choices stay");
    composer.apply(&world.ctx(), Pick::Model(Some("no-reasoning".into())));
    assert_eq!(composer.thinking, None);
    assert!(!composer.fields(&world.ctx()).contains(&Field::Thinking));
}

#[test]
fn model_choices_offer_typed_ids_only_when_the_cli_takes_one() {
    let world = world();
    let composer = settled(&world);
    let labels = |query: &str| -> Vec<String> {
        composer.choices(&world.ctx(), Field::Model, query).into_iter().map(|c| c.label).collect()
    };
    assert_eq!(labels(""), ["Default model", "Opus", "Sonnet"]);
    assert_eq!(labels("son"), ["Sonnet", "Use son"], "a known model ranks above typing your own");
    assert_eq!(labels("claude-opus-5-5"), ["Use claude-opus-5-5"]);
    assert!(labels("opus").contains(&"Opus".to_string()));
    assert!(!labels("opus").iter().any(|l| l.starts_with("Use ")), "an exact id needs no Use entry");
}

#[test]
fn workspace_choices_list_new_named_and_existing_checkouts() {
    let world = world();
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    composer.task.set("Fix login loop");
    let choices = composer.choices(&world.ctx(), Field::Workspace, "");
    let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["New worktree", "main", "fix-login"]);
    assert_eq!(choices[0].detail, "⎇ fix-login-loop");
    assert!(choices[1].detail.ends_with("main checkout"));
    let named = composer.choices(&world.ctx(), Field::Workspace, "feat/x");
    assert_eq!(named[0].pick, Pick::Workspace(WorkspaceSel::Named("feat/x".into())));
    let invalid = composer.choices(&world.ctx(), Field::Workspace, "bad name");
    assert!(!invalid.iter().any(|c| matches!(c.pick, Pick::Workspace(WorkspaceSel::Named(_)))));
}

#[test]
fn the_branch_preview_avoids_taken_branches_and_uses_the_prefix() {
    let mut world = world();
    world.config = Config::parse_toml("branch_prefix = \"lucas/\"", std::path::Path::new("c")).unwrap();
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    composer.task.set("main");
    assert_eq!(composer.branch_preview(&world.ctx()).as_deref(), Some("lucas/main"));
    world.config = Config::default();
    assert_eq!(composer.branch_preview(&world.ctx()).as_deref(), Some("main-2"), "main has a checkout");
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Checkout("/w/cockpit".into())));
    assert_eq!(composer.branch_preview(&world.ctx()), None);
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::New));
    composer.task.clear();
    assert_eq!(composer.branch_preview(&world.ctx()), None, "nothing to name yet");
}

#[test]
fn a_remembered_checkout_mode_selects_the_main_checkout() {
    let mut world = world();
    remember(&mut world, "cockpit", "local", "claude", "", "", "checkout");
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("api".into()));
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    assert_eq!(composer.workspace, WorkspaceSel::Checkout("/w/cockpit".into()));
    world.config.default_workspace = WorkspaceMode::Checkout;
    composer.apply(&world.ctx(), Pick::Project("api".into()));
    assert_eq!(
        composer.workspace,
        WorkspaceSel::Checkout("/w/api".into()),
        "the config default applies without history"
    );
}

#[test]
fn a_chosen_checkout_that_disappears_falls_back() {
    let world = world();
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Checkout("/gone".into())));
    composer.settle(&world.ctx());
    assert_eq!(composer.workspace, WorkspaceSel::New);
}

#[test]
fn a_valid_request_carries_every_choice() {
    let world = world();
    let mut composer = settled(&world);
    composer.task.set("  Fix login loop  ");
    composer.apply(&world.ctx(), Pick::Model(Some("opus".into())));
    composer.apply(&world.ctx(), Pick::Thinking(Some("high".into())));
    let request = composer.requests(&world.ctx()).unwrap().remove(0);
    assert_eq!(request.machine_label, "Local");
    assert_eq!(request.project.name, "api");
    assert_eq!(request.harness, "claude");
    assert_eq!(request.model.as_deref(), Some("opus"));
    assert_eq!(request.thinking.as_deref(), Some("high"));
    assert_eq!(request.workspace, WorkspaceChoice::NewWorktree { branch: None });
    assert_eq!(request.task, "  Fix login loop  ", "the plan trims, not the composer");
}

#[test]
fn values_read_naturally() {
    let world = world();
    let mut composer = settled(&world);
    assert_eq!(composer.value(&world.ctx(), Field::Model), "Default (opus)");
    composer.apply(&world.ctx(), Pick::Model(Some("sonnet".into())));
    assert_eq!(composer.value(&world.ctx(), Field::Model), "Sonnet");
    composer.apply(&world.ctx(), Pick::Model(Some("custom-1".into())));
    assert_eq!(composer.value(&world.ctx(), Field::Model), "custom-1");
    assert_eq!(composer.value(&world.ctx(), Field::Harness), "Claude");
    assert_eq!(composer.value(&world.ctx(), Field::Machine), "Local");
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Checkout("/h/wt/fix-login".into())));
    assert_eq!(composer.value(&world.ctx(), Field::Workspace), "Checkout · fix-login");
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Named("feat/a".into())));
    assert_eq!(composer.value(&world.ctx(), Field::Workspace), "New worktree · feat/a");
}

#[test]
fn with_nothing_discovered_the_composer_says_so_and_refuses_to_send() {
    let world = World {
        machines: vec![machine("local", "Local", Connection::Live)],
        inventories: HashMap::new(),
        preferences: Preferences::default(),
        config: Config::default(),
        presets: Vec::new(),
        usage: HashMap::new(),
    };
    let mut composer = settled(&world);
    assert_eq!(composer.project, None);
    assert_eq!(composer.value(&world.ctx(), Field::Project), "none found yet");
    composer.task.set("x");
    assert_eq!(composer.requests(&world.ctx()).unwrap_err(), "Pick a project.");
    let _ = ProjectChoices::default();
}

#[test]
fn function_keys_map_to_fields() {
    assert_eq!(Field::from_key(2), Some(Field::Project));
    assert_eq!(Field::from_key(9), Some(Field::Workspace));
    assert_eq!(Field::from_key(12), Some(Field::Compare));
    assert_eq!(Field::from_key(5), None, "F5 is not a field");
}

fn contender(harness: &str, model: Option<&str>, thinking: Option<&str>) -> Contender {
    Contender { harness: harness.into(), model: model.map(str::to_string), thinking: thinking.map(str::to_string) }
}

#[test]
fn comparing_is_off_until_an_agent_is_added_and_stops_at_three() {
    let mut world = world();
    world.presets = vec![
        Preset { name: "Opus high".into(), harness: "claude".into(), model: "opus".into(), thinking: "high".into() },
        Preset { name: "Studio only".into(), harness: "pi".into(), model: "x".into(), thinking: String::new() },
    ];
    let mut composer = settled(&world);
    assert!(!composer.is_comparing(), "one agent by default");
    assert_eq!(composer.value(&world.ctx(), Field::Compare), "Off · one agent");
    let labels: Vec<(String, bool)> =
        composer.choices(&world.ctx(), Field::Compare, "").into_iter().map(|c| (c.label, c.enabled)).collect();
    assert_eq!(
        labels,
        [("Off".to_string(), true), ("Opus high".into(), true), ("Codex".into(), true), ("Claude".into(), true)],
        "presets and harnesses installed here; the Pi preset is not"
    );

    composer.apply(&world.ctx(), Pick::CompareWith(contender("codex", None, None)));
    assert_eq!(composer.compare, [contender("codex", None, None)]);
    assert_eq!(composer.value(&world.ctx(), Field::Compare), "2 agents · also Codex");
    composer.apply(&world.ctx(), Pick::CompareWith(contender("claude", Some("opus"), Some("high"))));
    assert_eq!(composer.value(&world.ctx(), Field::Compare), "3 agents · also Codex, Claude Opus");
    let choices = composer.choices(&world.ctx(), Field::Compare, "");
    assert_eq!(choices[1].label, "✓ Opus high");
    assert!(choices[1].enabled, "an added agent can always be dropped");
    assert_eq!(choices[3].label, "Claude");
    assert!(!choices[3].enabled, "three agents at most");
    assert_eq!(choices[3].detail, "3 agents at most");
    composer.apply(&world.ctx(), Pick::CompareWith(contender("claude", None, None)));
    assert_eq!(composer.compare.len(), 2, "a fourth agent is refused");
    assert_eq!(composer.error.as_deref(), Some("3 agents at most."));

    composer.apply(&world.ctx(), Pick::CompareWith(contender("codex", None, None)));
    assert_eq!(composer.compare, [contender("claude", Some("opus"), Some("high"))], "picking again drops it");
    composer.apply(&world.ctx(), Pick::CompareNone);
    assert!(!composer.is_comparing());
}

#[test]
fn compared_agents_each_get_their_own_worktree_with_a_telling_branch() {
    let world = world();
    let mut composer = settled(&world);
    composer.apply(&world.ctx(), Pick::Project("cockpit".into()));
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Checkout("/w/cockpit".into())));
    composer.task.set("Fix login");
    composer.apply(&world.ctx(), Pick::Model(Some("claude-opus-5-5".into())));
    composer.apply(&world.ctx(), Pick::CompareWith(contender("codex", Some("gpt-5"), None)));
    assert_eq!(composer.workspace, WorkspaceSel::New, "a checkout cannot hold two agents");
    let choices = composer.choices(&world.ctx(), Field::Workspace, "");
    let checkouts: Vec<&Choice> =
        choices.iter().filter(|c| matches!(c.pick, Pick::Workspace(WorkspaceSel::Checkout(_)))).collect();
    assert!(!checkouts.is_empty() && checkouts.iter().all(|c| !c.enabled), "checkouts are shown but disabled");
    assert_eq!(checkouts[0].detail, "compared agents each need their own worktree");
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Checkout("/w/cockpit".into())));
    assert_eq!(composer.workspace, WorkspaceSel::New, "and refused if picked anyway");
    assert!(composer.error.is_some());

    assert_eq!(composer.branch_preview(&world.ctx()).as_deref(), Some("fix-login-{claude-opus-5-5,codex-gpt-5}"));
    let requests = composer.requests(&world.ctx()).unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].harness, "claude");
    assert_eq!(requests[0].model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        requests[0].workspace,
        WorkspaceChoice::NewWorktree { branch: Some("fix-login-claude-opus-5-5".into()) },
        "the harness is not repeated when the model id starts with it"
    );
    assert_eq!(requests[0].comparison, Some(Comparison { index: 0, total: 2 }));
    assert_eq!(requests[1].harness, "codex");
    assert_eq!(requests[1].workspace, WorkspaceChoice::NewWorktree { branch: Some("fix-login-codex-gpt-5".into()) });
    assert_eq!(requests[1].comparison, Some(Comparison { index: 1, total: 2 }));
    assert_eq!(requests[1].task, requests[0].task, "the same task for every agent");

    // A named branch is the stem; branches that exist get a number.
    composer.apply(&world.ctx(), Pick::Workspace(WorkspaceSel::Named("fix-login".into())));
    composer.apply(&world.ctx(), Pick::CompareWith(contender("claude", Some("claude-opus-5-5"), None)));
    let branches = composer.planned_branches(&world.ctx()).unwrap();
    assert_eq!(branches, ["fix-login-claude-opus-5-5", "fix-login-codex-gpt-5", "fix-login-claude-opus-5-5-2"]);
    assert_eq!(
        composer.branch_preview(&world.ctx()).as_deref(),
        Some("fix-login-{claude-opus-5-5,codex-gpt-5,claude-opus-5-5-2}")
    );
    let prefixed = World {
        config: Config {
            global: crate::config::MachineSettings { branch_prefix: Some("lucas/".into()), ..Default::default() },
            ..Config::default()
        },
        ..world
    };
    composer.apply(&prefixed.ctx(), Pick::Workspace(WorkspaceSel::New));
    assert_eq!(composer.planned_branches(&prefixed.ctx()).unwrap()[0], "lucas/fix-login-claude-opus-5-5");
}

#[test]
fn a_compared_agent_the_machine_lacks_is_dropped_when_the_machine_changes() {
    let world = world();
    let mut composer = settled(&world);
    composer.task.set("Fix it");
    composer.apply(&world.ctx(), Pick::CompareWith(contender("claude", None, None)));
    composer.apply(&world.ctx(), Pick::CompareWith(contender("codex", None, None)));
    composer.apply(&world.ctx(), Pick::Machine("studio".into()));
    assert_eq!(composer.compare, [contender("codex", None, None)], "the studio has no Claude");
    composer.compare.push(contender("claude", None, None));
    assert_eq!(
        composer.requests(&world.ctx()).unwrap_err(),
        "Claude is not installed on Mac Studio.",
        "nothing launches when one agent cannot"
    );
}

mod usage {
    use super::*;
    use crate::usage::{Report, Slot, Usage, Window};

    fn usage(windows: &[(&str, u32)], reached: bool) -> Usage {
        Usage {
            windows: windows
                .iter()
                .map(|(l, u)| Window { label: l.to_string(), used_percent: *u, resets_at: None })
                .collect(),
            reached,
            read_at: 900,
            ..Usage::default()
        }
    }

    fn report(entries: &[(&str, Option<Usage>)]) -> Report {
        Report {
            providers: entries
                .iter()
                .map(|(provider, usage)| {
                    let slot = match usage {
                        Some(usage) => Slot { usage: Some(usage.clone()), error: None },
                        None => Slot { usage: None, error: Some("no Claude thread has reported yet".into()) },
                    };
                    (provider.to_string(), slot)
                })
                .collect(),
            statusline: Some("/s/usage/statusline.py".into()),
            read_at: 900,
        }
    }

    /// Claude at 62 % left, Codex at 83 % left on the local machine.
    fn world_with_usage() -> World {
        let mut world = world();
        world.usage.insert(
            "local".into(),
            report(&[
                ("anthropic", Some(usage(&[("5h", 38), ("week", 29)], false))),
                ("openai", Some(usage(&[("week", 17)], false))),
            ]),
        );
        world
    }

    #[test]
    fn auto_picks_the_harness_with_the_most_usage_left_and_its_remembered_model() {
        let mut world = world_with_usage();
        remember(&mut world, "cockpit", "local", "claude", "sonnet", "high", "worktree");
        remember(&mut world, "cockpit", "local", "codex", "gpt-5", "", "worktree");
        let mut composer = settled(&world);
        assert_eq!(composer.harness.as_deref(), Some("codex"), "the last launch");
        composer.apply(&world.ctx(), Pick::AutoHarness);
        assert!(composer.auto_harness);
        assert_eq!(composer.harness.as_deref(), Some("codex"), "83 % left beats 62 %");
        assert_eq!(composer.model.as_deref(), Some("gpt-5"));
        assert_eq!(composer.value(&world.ctx(), Field::Harness), "Auto → Codex · 83% left");
        // The numbers change: Codex is nearly spent, Claude takes over with its own model.
        world.usage.insert(
            "local".into(),
            report(&[
                ("anthropic", Some(usage(&[("5h", 38), ("week", 29)], false))),
                ("openai", Some(usage(&[("week", 97)], false))),
            ]),
        );
        composer.settle(&world.ctx());
        assert_eq!(composer.harness.as_deref(), Some("claude"));
        assert_eq!(composer.model.as_deref(), Some("sonnet"));
        assert_eq!(composer.thinking.as_deref(), Some("high"));
        assert_eq!(composer.value(&world.ctx(), Field::Harness), "Auto → Claude · 62% left");
        // A limit reached is never picked, whatever the other window says.
        world.usage.insert(
            "local".into(),
            report(&[
                ("anthropic", Some(usage(&[("5h", 100)], true))),
                ("openai", Some(usage(&[("week", 97)], false))),
            ]),
        );
        composer.settle(&world.ctx());
        assert_eq!(composer.harness.as_deref(), Some("codex"));
        assert_eq!(composer.value(&world.ctx(), Field::Harness), "Auto → Codex · 3% left");
    }

    #[test]
    fn auto_waits_for_numbers_and_says_so() {
        let world = world();
        let mut composer = settled(&world);
        composer.apply(&world.ctx(), Pick::AutoHarness);
        assert_eq!(composer.harness.as_deref(), Some("claude"), "the preferred harness until usage is known");
        assert_eq!(composer.value(&world.ctx(), Field::Harness), "Auto no usage data yet");
        composer.task.set("Fix it");
        let request = &composer.requests(&world.ctx()).unwrap()[0];
        assert_eq!(request.harness, "claude", "sending never waits for usage");
        let mut spent = world;
        spent.usage.insert(
            "local".into(),
            report(&[
                ("anthropic", Some(usage(&[("5h", 100)], true))),
                ("openai", Some(usage(&[("week", 100)], true))),
            ]),
        );
        composer.settle(&spent.ctx());
        assert_eq!(composer.value(&spent.ctx(), Field::Harness), "Auto every limit reached");
        assert_eq!(composer.harness.as_deref(), Some("claude"), "still sendable: the provider decides");
    }

    #[test]
    fn the_harness_picker_offers_auto_first_and_shows_each_usage() {
        let world = world_with_usage();
        let composer = settled(&world);
        let choices = composer.choices(&world.ctx(), Field::Harness, "");
        let rows: Vec<(String, String)> = choices.iter().map(|c| (c.label.clone(), c.detail.clone())).collect();
        assert_eq!(
            rows,
            [
                ("Auto · most usage left".to_string(), "→ Codex · 83% left".to_string()),
                ("Codex".to_string(), "week 17%".to_string()),
                ("Claude".to_string(), "5h 38% · week 29%".to_string()),
            ]
        );
        assert_eq!(choices[0].pick, Pick::AutoHarness);
        assert_eq!(
            composer.value(&world.ctx(), Field::Harness),
            "Claude · 62% left",
            "a named harness shows its headroom"
        );
        let mut without = world_with_usage();
        without
            .usage
            .insert("local".into(), report(&[("anthropic", None), ("openai", Some(usage(&[("week", 17)], false)))]));
        let choices = composer.choices(&without.ctx(), Field::Harness, "");
        assert_eq!(choices[2].detail, "no Claude thread has reported yet");
        let mut off = world_with_usage();
        off.config.usage.enabled = false;
        let choices = composer.choices(&off.ctx(), Field::Harness, "");
        assert!(choices.iter().all(|c| c.pick != Pick::AutoHarness), "no Auto when usage is off");
        assert_eq!(choices[0].detail, "codex", "the raw kind, as before");
        assert_eq!(composer.value(&off.ctx(), Field::Harness), "Claude");
        let mut studio = world_with_usage();
        studio.usage.remove("local");
        let mut composer = settled(&studio);
        composer.apply(&studio.ctx(), Pick::Machine("studio".into()));
        let choices = composer.choices(&studio.ctx(), Field::Harness, "");
        assert_eq!(choices[0].detail, "no usage data yet", "nothing read on the studio yet");
        assert_eq!(choices[1].detail, "no usage data yet");
    }

    #[test]
    fn a_project_that_remembers_auto_starts_on_auto_and_skips_the_first_preset() {
        let mut world = world_with_usage();
        world.presets.push(Preset {
            name: "Opus".into(),
            harness: "claude".into(),
            model: "opus".into(),
            thinking: String::new(),
        });
        world.preferences.remember(&Remembered {
            auto: true,
            project: "cockpit".into(),
            machine: "local".into(),
            harness: "claude".into(),
            workspace: "worktree".into(),
            model: "sonnet".into(),
            thinking: String::new(),
        });
        assert_eq!(world.preferences.project("cockpit").harness.as_deref(), Some(AUTO));
        assert_eq!(
            world.preferences.project("cockpit").models["local"]["claude"],
            "sonnet",
            "under the harness that ran"
        );
        let mut composer = Composer { start_preset: true, ..Composer::default() };
        composer.settle(&world.ctx());
        assert!(composer.auto_harness, "the project asked the inbox to choose");
        assert!(!composer.start_preset);
        assert_eq!(composer.harness.as_deref(), Some("codex"), "not the first preset's Claude");
        // Naming a harness or a preset ends the choosing.
        composer.apply(&world.ctx(), Pick::Preset("Opus".into()));
        assert!(!composer.auto_harness);
        assert_eq!(composer.harness.as_deref(), Some("claude"));
        composer.apply(&world.ctx(), Pick::AutoHarness);
        assert!(composer.auto_harness);
        composer.apply(&world.ctx(), Pick::Harness("claude".into()));
        assert!(!composer.auto_harness);
        assert_eq!(composer.value(&world.ctx(), Field::Harness), "Claude · 62% left");
        // A machine without usage is settled as before.
        composer.apply(&world.ctx(), Pick::AutoHarness);
        composer.apply(&world.ctx(), Pick::Machine("studio".into()));
        assert!(composer.auto_harness, "auto survives a machine change");
        assert_eq!(composer.harness.as_deref(), Some("codex"), "the only harness there");
        let mut off = world_with_usage();
        off.config.usage.enabled = false;
        let mut composer = Composer { start_preset: true, ..Composer::default() };
        composer.settle(&off.ctx());
        assert!(!composer.auto_harness, "usage off: the remembered auto is ignored");
        assert_eq!(composer.harness.as_deref(), Some("claude"), "the first preset, as before");
    }

    #[test]
    fn a_configured_provider_lends_its_usage_to_another_harness() {
        let mut world = world_with_usage();
        world.config.usage.providers.insert("codex".into(), "anthropic".into());
        let mut composer = settled(&world);
        composer.apply(&world.ctx(), Pick::AutoHarness);
        assert_eq!(composer.harness.as_deref(), Some("claude"), "both at 62 %: the preferred order");
        let choices = composer.choices(&world.ctx(), Field::Harness, "");
        assert_eq!(choices[1].detail, "5h 38% · week 29%", "Codex shows the Anthropic numbers");
    }
}
