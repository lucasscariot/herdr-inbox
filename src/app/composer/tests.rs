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
}

impl World {
    fn ctx(&self) -> Context<'_> {
        Context {
            machines: &self.machines,
            inventories: &self.inventories,
            preferences: &self.preferences,
            config: &self.config,
            presets: &self.presets,
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
