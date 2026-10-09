use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::discovery::{CheckoutEntry, Choice as ModelChoice};
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
}

impl World {
    fn ctx(&self) -> Context<'_> {
        Context {
            machines: &self.machines,
            inventories: &self.inventories,
            preferences: &self.preferences,
            config: &self.config,
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
        },
    );
    inventories.insert(
        "studio".to_string(),
        Inventory {
            projects: vec![project("api", &[("main", false)])],
            harnesses: vec!["codex".into()],
            models: BTreeMap::from([("codex".into(), codex())]),
            models_at: 1,
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
    let err = composer.request(&world.ctx()).unwrap_err();
    assert_eq!(err, "Write a task first.");
    composer.task.set("Fix it");
    assert_eq!(composer.request(&world.ctx()).unwrap_err(), "MacBook is not reachable right now.");
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
    assert_eq!(composer.field, Field::Task, "wraps around");
    composer.next_field(&world.ctx(), false);
    assert_eq!(composer.field, Field::Workspace);
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
    let request = composer.request(&world.ctx()).unwrap();
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
    };
    let mut composer = settled(&world);
    assert_eq!(composer.project, None);
    assert_eq!(composer.value(&world.ctx(), Field::Project), "none found yet");
    composer.task.set("x");
    assert_eq!(composer.request(&world.ctx()).unwrap_err(), "Pick a project.");
    let _ = ProjectChoices::default();
}

#[test]
fn function_keys_map_to_fields() {
    assert_eq!(Field::from_key(2), Some(Field::Project));
    assert_eq!(Field::from_key(9), Some(Field::Workspace));
    assert_eq!(Field::from_key(5), None, "F5 is not a field");
}
