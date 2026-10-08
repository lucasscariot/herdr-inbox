//! The composer: write a task, choose where and how it runs, send it.
//!
//! Choices cascade: a project picks its remembered machine, the machine a
//! harness installed there, the harness its remembered model and thinking
//! level for that machine. A choice that stops being valid (the machine lost
//! the project, the harness is not installed there) falls back to a valid one.

use std::collections::HashMap;

use crate::config::{Config, WorkspaceMode};
use crate::discovery::{Catalog, Inventory, Project};
use crate::editor::Editor;
use crate::launch::{self, Request, WorkspaceChoice};
use crate::state::Preferences;
use crate::threads::harness_label_for;

use super::{Connection, MachineState};

/// Harnesses offered first when a project has no remembered one.
const PREFERRED: &[&str] = &["claude", "codex", "opencode", "pi", "gemini"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Task,
    Project,
    Machine,
    Harness,
    Model,
    Thinking,
    Workspace,
}

impl Field {
    pub const ORDER: [Field; 7] =
        [Field::Task, Field::Project, Field::Machine, Field::Harness, Field::Model, Field::Thinking, Field::Workspace];

    pub fn label(self) -> &'static str {
        match self {
            Field::Task => "Task",
            Field::Project => "Project",
            Field::Machine => "Machine",
            Field::Harness => "Harness",
            Field::Model => "Model",
            Field::Thinking => "Thinking",
            Field::Workspace => "Workspace",
        }
    }

    /// The function key that opens this field's picker.
    pub fn key(self) -> Option<u8> {
        match self {
            Field::Task => None,
            Field::Project => Some(2),
            Field::Harness => Some(3),
            Field::Model => Some(4),
            Field::Machine => Some(6),
            Field::Thinking => Some(8),
            Field::Workspace => Some(9),
        }
    }

    pub fn from_key(n: u8) -> Option<Field> {
        Field::ORDER.into_iter().find(|f| f.key() == Some(n))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSel {
    /// A new worktree, its branch named after the task.
    New,
    /// A new worktree with a branch the user typed.
    Named(String),
    /// An existing checkout, by path.
    Checkout(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Project(String),
    Machine(String),
    Harness(String),
    Model(Option<String>),
    Thinking(Option<String>),
    Workspace(WorkspaceSel),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    pub pick: Pick,
    /// A choice shown but not selectable, such as an unreachable machine.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub field: Field,
    pub query: String,
    pub selected: usize,
    /// Where focus goes back when the picker closes: the task, when it was
    /// opened with a function key while typing.
    pub return_to: Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    pub task: Editor,
    pub project: Option<String>,
    pub machine: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub workspace: WorkspaceSel,
    pub field: Field,
    pub picker: Option<Picker>,
    /// Why the last send was refused.
    pub error: Option<String>,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            task: Editor::default(),
            project: None,
            machine: None,
            harness: None,
            model: None,
            thinking: None,
            workspace: WorkspaceSel::New,
            field: Field::Task,
            picker: None,
            error: None,
        }
    }
}

/// What the composer reads from the rest of the app.
pub struct Context<'a> {
    pub machines: &'a [MachineState],
    pub inventories: &'a HashMap<String, Inventory>,
    pub preferences: &'a Preferences,
    pub config: &'a Config,
}

impl Context<'_> {
    fn inventory(&self, machine: &str) -> Option<&Inventory> {
        self.inventories.get(machine)
    }

    fn machine(&self, id: &str) -> Option<&MachineState> {
        self.machines.iter().find(|m| m.id == id)
    }

    /// Machines whose inventory has the project, in machine order.
    pub fn machines_with(&self, project: &str) -> Vec<&MachineState> {
        self.machines.iter().filter(|m| self.inventory(&m.id).is_some_and(|i| i.project(project).is_some())).collect()
    }

    /// Every project name known on any machine, the last used one first.
    pub fn project_names(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.inventories.values().flat_map(|i| i.projects.iter().map(|p| p.name.clone())).collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        if let Some(last) = self.preferences.last_project.as_ref().filter(|l| names.contains(l)) {
            names.retain(|n| n != last);
            names.insert(0, last.clone());
        }
        names
    }

    pub fn project(&self, composer: &Composer) -> Option<&Project> {
        self.inventory(composer.machine.as_deref()?)?.project(composer.project.as_deref()?)
    }

    pub fn catalog(&self, composer: &Composer) -> Option<&Catalog> {
        self.inventory(composer.machine.as_deref()?)?.models.get(composer.harness.as_deref()?)
    }

    fn installed(&self, machine: &str) -> Vec<String> {
        self.inventory(machine).map(|i| i.harnesses.clone()).unwrap_or_default()
    }
}

impl Composer {
    /// Fields worth showing: Thinking only when the harness takes a level.
    pub fn fields(&self, ctx: &Context) -> Vec<Field> {
        let thinking = ctx.catalog(self).is_some_and(|c| !c.thinking.is_empty());
        Field::ORDER.into_iter().filter(|f| *f != Field::Thinking || thinking).collect()
    }

    pub fn next_field(&mut self, ctx: &Context, forward: bool) {
        let fields = self.fields(ctx);
        let index = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        let next = if forward { (index + 1) % fields.len() } else { (index + fields.len() - 1) % fields.len() };
        self.field = fields[next];
    }

    pub fn open_picker(&mut self, field: Field, query: &str) {
        if field == Field::Task {
            return;
        }
        let return_to = self.field;
        self.field = field;
        self.picker = Some(Picker { field, query: query.to_string(), selected: 0, return_to });
    }

    /// Closes the picker, focus back where it came from.
    pub fn close_picker(&mut self) {
        if let Some(picker) = self.picker.take() {
            self.field = picker.return_to;
        }
    }

    /// Fills empty or invalid choices from what the user did last time.
    pub fn settle(&mut self, ctx: &Context) {
        let names = ctx.project_names();
        if self.project.as_ref().is_none_or(|p| !names.contains(p)) {
            let remembered = ctx.preferences.last_project.clone().filter(|p| names.contains(p));
            self.project = remembered.or_else(|| names.first().cloned());
            if self.project.is_some() {
                self.machine = None;
            }
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let remembered = ctx.preferences.project(&project);
        let hosts: Vec<String> = ctx.machines_with(&project).iter().map(|m| m.id.clone()).collect();
        if self.machine.as_ref().is_none_or(|m| !hosts.contains(m)) {
            let live = |id: &String| ctx.machine(id).is_some_and(|m| m.connection.is_live());
            self.machine = remembered
                .machine
                .clone()
                .filter(|m| hosts.contains(m))
                .or_else(|| hosts.iter().find(|h| live(h)).cloned())
                .or_else(|| hosts.first().cloned());
            self.harness = None;
        }
        let Some(machine) = self.machine.clone() else {
            return;
        };
        let installed = ctx.installed(&machine);
        if self.harness.as_ref().is_none_or(|h| !installed.contains(h)) {
            self.harness = remembered
                .harness
                .clone()
                .filter(|h| installed.contains(h))
                .or_else(|| PREFERRED.iter().find(|p| installed.iter().any(|i| i == *p)).map(|p| p.to_string()))
                .or_else(|| installed.first().cloned());
            self.model = remembered.models.get(&machine).and_then(|m| m.get(self.harness.as_deref()?)).cloned();
            self.thinking = remembered.thinking.get(&machine).and_then(|m| m.get(self.harness.as_deref()?)).cloned();
        }
        if let Some(catalog) = ctx.catalog(self).cloned() {
            if self.model.is_some() && !catalog.selectable {
                self.model = None;
            }
            if self.thinking.as_ref().is_some_and(|t| !catalog.thinking.contains(t)) {
                self.thinking = None;
            }
        }
        if let WorkspaceSel::Checkout(path) = &self.workspace {
            let exists =
                ctx.project(self).is_some_and(|p| p.checkouts.iter().any(|c| &c.path == path) || &p.path == path);
            if !exists {
                self.workspace = self.default_workspace(ctx);
            }
        }
        if !self.fields(ctx).contains(&self.field) {
            self.field = Field::Task;
        }
    }

    fn default_workspace(&self, ctx: &Context) -> WorkspaceSel {
        let remembered = self.project.as_deref().and_then(|p| ctx.preferences.project(p).workspace);
        let mode = match remembered.as_deref() {
            Some("checkout") => WorkspaceMode::Checkout,
            Some("worktree") => WorkspaceMode::Worktree,
            _ => ctx.config.default_workspace,
        };
        match (mode, ctx.project(self)) {
            (WorkspaceMode::Checkout, Some(project)) => WorkspaceSel::Checkout(project.main_checkout().path),
            _ => WorkspaceSel::New,
        }
    }

    /// Applies a picked choice and re-settles everything after it.
    pub fn apply(&mut self, ctx: &Context, pick: Pick) {
        match pick {
            Pick::Project(name) => {
                if self.project.as_deref() != Some(&name) {
                    self.project = Some(name);
                    self.machine = None;
                    self.harness = None;
                    self.workspace = WorkspaceSel::New;
                    self.settle(ctx);
                    self.workspace = self.default_workspace(ctx);
                }
            }
            Pick::Machine(id) => {
                if self.machine.as_deref() != Some(&id) {
                    self.machine = Some(id);
                    self.harness = None;
                    self.settle(ctx);
                }
            }
            Pick::Harness(kind) => {
                if self.harness.as_deref() != Some(&kind) {
                    let machine = self.machine.clone().unwrap_or_default();
                    let remembered = self.project.as_deref().map(|p| ctx.preferences.project(p)).unwrap_or_default();
                    self.model = remembered.models.get(&machine).and_then(|m| m.get(&kind)).cloned();
                    self.thinking = remembered.thinking.get(&machine).and_then(|m| m.get(&kind)).cloned();
                    self.harness = Some(kind);
                    self.settle(ctx);
                }
            }
            Pick::Model(model) => self.model = model,
            Pick::Thinking(level) => self.thinking = level,
            Pick::Workspace(workspace) => self.workspace = workspace,
        }
        self.error = None;
    }

    /// The branch a new worktree would get right now.
    pub fn branch_preview(&self, ctx: &Context) -> Option<String> {
        match &self.workspace {
            WorkspaceSel::Named(name) => Some(name.clone()),
            // No task, no name worth showing yet.
            WorkspaceSel::New if self.task.is_blank() => None,
            WorkspaceSel::New => {
                let project = ctx.project(self)?;
                let machine = ctx.machine(self.machine.as_deref()?)?;
                let prefix = ctx.config.for_machine(&machine.id, &machine.label, machine.is_local()).branch_prefix;
                Some(launch::branch_name(self.task.text(), &project.taken_branches(), &prefix))
            }
            WorkspaceSel::Checkout(_) => None,
        }
    }

    /// Every choice for a field, best match for the picker's query first.
    pub fn choices(&self, ctx: &Context, field: Field, query: &str) -> Vec<Choice> {
        let all = self.all_choices(ctx, field, query);
        crate::fuzzy::rank(query, &all, |c| c.label.clone()).into_iter().cloned().collect()
    }

    fn all_choices(&self, ctx: &Context, field: Field, query: &str) -> Vec<Choice> {
        let choice = |label: String, detail: String, pick: Pick| Choice { label, detail, pick, enabled: true };
        match field {
            Field::Task => Vec::new(),
            Field::Project => ctx
                .project_names()
                .into_iter()
                .map(|name| {
                    let hosts: Vec<String> = ctx.machines_with(&name).iter().map(|m| m.label.clone()).collect();
                    choice(name.clone(), hosts.join(", "), Pick::Project(name))
                })
                .collect(),
            Field::Machine => {
                let Some(project) = self.project.as_deref() else {
                    return Vec::new();
                };
                ctx.machines_with(project)
                    .into_iter()
                    .map(|m| {
                        let (detail, enabled) = match &m.connection {
                            Connection::Live => (String::new(), true),
                            Connection::Connecting | Connection::Starting(_) => ("connecting".into(), false),
                            Connection::NoServer => ("no Herdr server".into(), false),
                            Connection::Lost(_) => ("unreachable".into(), false),
                        };
                        Choice { label: m.label.clone(), detail, pick: Pick::Machine(m.id.clone()), enabled }
                    })
                    .collect()
            }
            Field::Harness => self
                .machine
                .as_deref()
                .map(|m| ctx.installed(m))
                .unwrap_or_default()
                .into_iter()
                .map(|kind| choice(harness_label_for(&kind), kind.clone(), Pick::Harness(kind)))
                .collect(),
            Field::Model => {
                let catalog = ctx.catalog(self).cloned().unwrap_or_default();
                let default_detail = if catalog.default.is_empty() { String::new() } else { catalog.default.clone() };
                let mut choices = vec![choice("Default model".into(), default_detail, Pick::Model(None))];
                choices.extend(
                    catalog
                        .choices
                        .iter()
                        .map(|c| choice(c.label.clone(), c.id.clone(), Pick::Model(Some(c.id.clone())))),
                );
                let typed = query.trim();
                if catalog.selectable && !typed.is_empty() && !catalog.choices.iter().any(|c| c.id == typed) {
                    choices.push(choice(
                        format!("Use {typed}"),
                        "a model id the CLI accepts".into(),
                        Pick::Model(Some(typed.into())),
                    ));
                }
                if !catalog.selectable {
                    for c in choices.iter_mut().skip(1) {
                        c.enabled = false;
                        c.detail = "this CLI picks its own model".into();
                    }
                }
                choices
            }
            Field::Thinking => {
                let catalog = ctx.catalog(self).cloned().unwrap_or_default();
                let mut choices = vec![choice("Default thinking".into(), String::new(), Pick::Thinking(None))];
                choices.extend(
                    catalog
                        .thinking
                        .iter()
                        .map(|level| choice(level.clone(), String::new(), Pick::Thinking(Some(level.clone())))),
                );
                choices
            }
            Field::Workspace => {
                let preview = match &self.workspace {
                    WorkspaceSel::New => self.branch_preview(ctx),
                    _ => {
                        let mut new = self.clone();
                        new.workspace = WorkspaceSel::New;
                        new.branch_preview(ctx)
                    }
                };
                let mut choices = vec![choice(
                    "New worktree".into(),
                    preview.map(|b| format!("⎇ {b}")).unwrap_or_default(),
                    Pick::Workspace(WorkspaceSel::New),
                )];
                let typed = query.trim();
                if !typed.is_empty() && launch::validate_branch(typed).is_ok() {
                    choices.push(choice(
                        format!("New worktree named {typed}"),
                        String::new(),
                        Pick::Workspace(WorkspaceSel::Named(typed.into())),
                    ));
                }
                if let Some(project) = ctx.project(self) {
                    let mut checkouts = project.checkouts.clone();
                    if checkouts.is_empty() {
                        checkouts.push(project.main_checkout());
                    }
                    for checkout in checkouts {
                        let label =
                            if checkout.branch.is_empty() { "checkout".to_string() } else { checkout.branch.clone() };
                        let detail = if checkout.linked {
                            checkout.path.clone()
                        } else {
                            format!("{} · main checkout", checkout.path)
                        };
                        choices.push(choice(label, detail, Pick::Workspace(WorkspaceSel::Checkout(checkout.path))));
                    }
                }
                choices
            }
        }
    }

    /// A launch request, or why it cannot be sent yet.
    pub fn request(&self, ctx: &Context) -> Result<Request, String> {
        if self.task.is_blank() {
            return Err("Write a task first.".into());
        }
        let project_name = self.project.clone().ok_or("Pick a project.")?;
        let machine_id = self.machine.clone().ok_or("Pick a machine that has this project.")?;
        let machine = ctx.machine(&machine_id).ok_or("That machine is gone.")?;
        if !machine.connection.is_live() {
            return Err(format!("{} is not reachable right now.", machine.label));
        }
        let harness = self.harness.clone().ok_or(format!("No agent CLI is installed on {}.", machine.label))?;
        let project = ctx.project(self).cloned().ok_or(format!("{project_name} is not on {}.", machine.label))?;
        let workspace = match &self.workspace {
            WorkspaceSel::New => WorkspaceChoice::NewWorktree { branch: None },
            WorkspaceSel::Named(name) => WorkspaceChoice::NewWorktree { branch: Some(name.clone()) },
            WorkspaceSel::Checkout(path) => WorkspaceChoice::Checkout { path: path.clone() },
        };
        Ok(Request {
            machine_id,
            machine_label: machine.label.clone(),
            project,
            harness,
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            task: self.task.text().to_string(),
            workspace,
        })
    }

    /// The value shown for a field.
    pub fn value(&self, ctx: &Context, field: Field) -> String {
        match field {
            Field::Task => self.task.text().to_string(),
            Field::Project => self.project.clone().unwrap_or_else(|| "none found yet".into()),
            Field::Machine => self
                .machine
                .as_deref()
                .and_then(|m| ctx.machine(m))
                .map(|m| m.label.clone())
                .unwrap_or_else(|| "—".into()),
            Field::Harness => self.harness.as_deref().map(harness_label_for).unwrap_or_else(|| "none installed".into()),
            Field::Model => match (&self.model, ctx.catalog(self)) {
                (Some(model), Some(catalog)) => catalog
                    .choices
                    .iter()
                    .find(|c| &c.id == model)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| model.clone()),
                (Some(model), None) => model.clone(),
                (None, Some(catalog)) if !catalog.default.is_empty() => format!("Default ({})", catalog.default),
                (None, _) => "Default".into(),
            },
            Field::Thinking => self.thinking.clone().unwrap_or_else(|| "Default".into()),
            Field::Workspace => match &self.workspace {
                WorkspaceSel::New => "New worktree".into(),
                WorkspaceSel::Named(name) => format!("New worktree · {name}"),
                WorkspaceSel::Checkout(path) => {
                    let branch = ctx
                        .project(self)
                        .and_then(|p| p.checkouts.iter().find(|c| &c.path == path))
                        .map(|c| c.branch.clone())
                        .filter(|b| !b.is_empty());
                    match branch {
                        Some(branch) => format!("Checkout · {branch}"),
                        None => "Checkout".into(),
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
