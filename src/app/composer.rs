//! The composer: write a task, choose where and how it runs, send it.
//!
//! Choices cascade: a project picks its remembered machine, the machine a
//! harness installed there, the harness its remembered model and thinking
//! level for that machine. A choice that stops being valid (the machine lost
//! the project, the harness is not installed there) falls back to a valid one.
//!
//! Presets sit right under the task as a strip of chips, in the user's order
//! of preference: a new thread starts on the first one, and a chip applies
//! its harness, model and thinking level in one move.
//!
//! Off by default, a task can also go to one or two more agents at once, to
//! compare their results. Each compared agent gets its own worktree, so they
//! never touch each other's files; the comparison is used once and cleared.

pub mod layout;

use std::collections::HashMap;

use crate::config::{Config, WorkspaceMode};
use crate::discovery::{Catalog, Inventory, Project};
use crate::editor::Editor;
use crate::launch::{self, Comparison, Request, WorkspaceChoice};
use crate::presets::{self, Preset};
use crate::state::Preferences;
use crate::threads::harness_label_for;

use super::{Connection, MachineState};

/// Harnesses offered first when a project has no remembered one.
const PREFERRED: &[&str] = &["claude", "codex", "opencode", "pi", "gemini"];
/// How many agents one task can go to at once.
pub const COMPARE_LIMIT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Task,
    /// The preset strip under the task.
    Preset,
    Project,
    Machine,
    Harness,
    Model,
    Thinking,
    Workspace,
    /// More agents the same task goes to, each in its own worktree.
    Compare,
}

impl Field {
    pub const ORDER: [Field; 9] = [
        Field::Task,
        Field::Preset,
        Field::Project,
        Field::Machine,
        Field::Harness,
        Field::Model,
        Field::Thinking,
        Field::Workspace,
        Field::Compare,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Task => "Task",
            Field::Project => "Project",
            Field::Machine => "Machine",
            Field::Preset => "Presets",
            Field::Harness => "Harness",
            Field::Model => "Model",
            Field::Thinking => "Thinking",
            Field::Workspace => "Workspace",
            Field::Compare => "Compare",
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
            Field::Preset => Some(7),
            Field::Thinking => Some(8),
            Field::Workspace => Some(9),
            Field::Compare => Some(12),
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

/// An agent a task can go to: a harness with a model and thinking level, or
/// a harness with its own defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contender {
    pub harness: String,
    pub model: Option<String>,
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Project(String),
    Machine(String),
    Harness(String),
    Model(Option<String>),
    Thinking(Option<String>),
    Workspace(WorkspaceSel),
    /// Back to one agent.
    CompareNone,
    /// Adds this agent to the comparison, or removes it when it is in.
    CompareWith(Contender),
    Preset(String),
    /// Save the current harness, model and thinking under this name.
    SavePreset(String),
    RenamePreset {
        from: String,
        to: String,
    },
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
    /// A preset prompt: the preset being renamed.
    pub renaming: Option<String>,
    /// A preset prompt: naming the preset about to be saved.
    pub saving: bool,
}

impl Picker {
    /// What the popup is for, as its title.
    pub fn title(&self) -> String {
        match (self.field, self.saving, &self.renaming) {
            (Field::Preset, true, _) => "Save preset".into(),
            (Field::Preset, _, Some(_)) => "Rename preset".into(),
            (field, _, _) => field.label().into(),
        }
    }
}

/// One chip in the preset strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// A saved preset, by its position.
    Preset(usize),
    /// Saves the current harness, model and thinking as a new preset.
    Save,
}

/// A chip ready to draw or click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipView {
    pub chip: Chip,
    pub label: String,
    /// The harness whose mark goes before the label.
    pub harness: Option<String>,
    /// A preset for a harness not installed on this machine is shown, dimmed.
    pub enabled: bool,
    /// The preset the current choices match.
    pub active: bool,
}

impl ChipView {
    /// Columns the chip takes: a space each side, the mark and its space.
    pub fn width(&self) -> u16 {
        use unicode_width::UnicodeWidthStr;
        2 + self.label.width() as u16 + if self.harness.is_some() { 2 } else { 0 }
    }
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
    /// Position in the task history while browsing it; `None` when editing.
    pub history_index: Option<usize>,
    /// The task being written before browsing the history.
    pub history_draft: String,
    /// Pasted images, shown in the task as `[Image #N]`.
    pub images: Vec<String>,
    /// Other agents the task also goes to, for comparison. Empty: one agent.
    pub compare: Vec<Contender>,
    /// The chip under the cursor while the preset strip has focus.
    pub preset_cursor: usize,
    /// Where focus goes after a chip is applied: the task when the strip
    /// was reached with F7 while writing, the strip itself when reached
    /// with Tab.
    pub strip_return: Field,
    /// A new thread starts on the first preset: applied by every `settle`
    /// that knows what the machine has installed, until discovery answers
    /// afresh or the user picks a harness, model, thinking or preset by
    /// hand. A cached inventory alone never settles the choice.
    pub start_preset: bool,
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
            history_index: None,
            history_draft: String::new(),
            images: Vec::new(),
            compare: Vec::new(),
            preset_cursor: 0,
            strip_return: Field::Preset,
            start_preset: false,
        }
    }
}

/// What the composer reads from the rest of the app.
pub struct Context<'a> {
    pub machines: &'a [MachineState],
    pub inventories: &'a HashMap<String, Inventory>,
    pub preferences: &'a Preferences,
    pub config: &'a Config,
    pub presets: &'a [Preset],
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
    /// Keyboard-reachable fields: Thinking only when the model reports levels.
    pub fn fields(&self, ctx: &Context) -> Vec<Field> {
        let thinking = ctx.catalog(self).is_some_and(|c| !c.thinking_levels(self.model.as_deref()).is_empty());
        Field::ORDER.into_iter().filter(|f| *f != Field::Thinking || thinking).collect()
    }

    pub fn next_field(&mut self, ctx: &Context, forward: bool) {
        let fields = self.fields(ctx);
        let index = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        let next = if forward { (index + 1) % fields.len() } else { (index + fields.len() - 1) % fields.len() };
        match fields[next] {
            Field::Preset => {
                self.field = Field::Preset;
                self.focus_presets(ctx);
            }
            field => self.field = field,
        }
    }

    /// Leaves the strip after a chip was applied.
    pub fn leave_presets(&mut self) {
        if self.field == Field::Preset {
            self.field = self.strip_return;
        }
    }

    /// Focuses the preset strip, the cursor on the preset in use. Applying
    /// a chip then goes back to the field focus came from.
    pub fn focus_presets(&mut self, ctx: &Context) {
        self.strip_return = self.field;
        self.field = Field::Preset;
        let active = self.matching_preset(ctx.presets).map(|p| p.name.clone());
        self.preset_cursor = ctx.presets.iter().position(|p| Some(&p.name) == active.as_ref()).unwrap_or(0);
    }

    /// Moves the strip cursor, wrapping around the ends.
    pub fn move_preset_cursor(&mut self, ctx: &Context, forward: bool) {
        let count = self.chips(ctx).len();
        let cursor = self.preset_cursor.min(count - 1);
        self.preset_cursor = if forward { (cursor + 1) % count } else { (cursor + count - 1) % count };
    }

    /// The preset strip: every preset in order of preference, then the save
    /// action. A strip with no presets is only the save action.
    pub fn chips(&self, ctx: &Context) -> Vec<ChipView> {
        let installed = self.machine.as_deref().map(|m| ctx.installed(m)).unwrap_or_default();
        let active = self.matching_preset(ctx.presets).map(|p| p.name.clone());
        let mut chips: Vec<ChipView> = ctx
            .presets
            .iter()
            .enumerate()
            .map(|(index, preset)| ChipView {
                chip: Chip::Preset(index),
                label: preset.name.clone(),
                harness: Some(preset.harness.clone()),
                enabled: installed.contains(&preset.harness),
                active: active.as_deref() == Some(preset.name.as_str()),
            })
            .collect();
        chips.push(ChipView {
            chip: Chip::Save,
            label: "+ Save as preset…".into(),
            harness: None,
            enabled: true,
            active: false,
        });
        chips
    }

    /// The chip under the strip cursor.
    pub fn chip_at_cursor(&self, ctx: &Context) -> ChipView {
        let chips = self.chips(ctx);
        let index = self.preset_cursor.min(chips.len() - 1);
        chips[index].clone()
    }

    /// Describes the chip under the cursor: the preset's harness, model and
    /// thinking, or why it cannot be picked.
    pub fn chip_detail(&self, ctx: &Context, chip: &ChipView) -> String {
        match chip.chip {
            Chip::Save => "the current harness, model and thinking, as a new chip".into(),
            Chip::Preset(_) if !chip.enabled => "not installed on this machine".into(),
            Chip::Preset(index) => {
                let preset = &ctx.presets[index];
                let thinking =
                    if preset.thinking.is_empty() { String::new() } else { format!(" · {}", preset.thinking) };
                format!("{} · {}{thinking}", harness_label_for(&preset.harness), preset.model)
            }
        }
    }

    pub fn open_picker(&mut self, field: Field, query: &str) {
        if field == Field::Task {
            return;
        }
        let return_to = self.field;
        self.field = field;
        self.picker =
            Some(Picker { field, query: query.to_string(), selected: 0, return_to, renaming: None, saving: false });
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
        if names.is_empty() {
            // Nothing discovered yet: keep what the user (or a re-send) chose.
            return;
        }
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
        if self.start_preset
            && let Some(preset) = ctx.presets.iter().find(|p| installed.contains(&p.harness))
        {
            self.harness = Some(preset.harness.clone());
            self.model = Some(preset.model.clone());
            self.thinking = Some(preset.thinking.clone()).filter(|t| !t.is_empty());
        }
        if let Some(catalog) = ctx.catalog(self).cloned() {
            if self.model.is_some() && !catalog.selectable {
                self.model = None;
            }
            if self.thinking.as_ref().is_some_and(|t| !catalog.thinking_levels(self.model.as_deref()).contains(t)) {
                self.thinking = None;
            }
        }
        self.compare.retain(|c| installed.contains(&c.harness));
        if let WorkspaceSel::Checkout(path) = &self.workspace {
            let exists =
                ctx.project(self).is_some_and(|p| p.checkouts.iter().any(|c| &c.path == path) || &p.path == path);
            if !exists {
                self.workspace = self.default_workspace(ctx);
            }
        }
        if self.is_comparing() && matches!(self.workspace, WorkspaceSel::Checkout(_)) {
            // Compared agents never share a checkout.
            self.workspace = WorkspaceSel::New;
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
        if matches!(pick, Pick::Harness(_) | Pick::Model(_) | Pick::Thinking(_) | Pick::Preset(_)) {
            // A choice made by hand outranks the first-preset default.
            self.start_preset = false;
        }
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
            Pick::Model(model) => {
                self.model = model;
                self.settle(ctx);
            }
            Pick::Thinking(level) => self.thinking = level,
            Pick::Workspace(workspace) => {
                if self.is_comparing() && matches!(workspace, WorkspaceSel::Checkout(_)) {
                    self.error = Some("Compared agents each need their own worktree.".into());
                    return;
                }
                self.workspace = workspace;
            }
            Pick::CompareNone => self.compare.clear(),
            Pick::CompareWith(contender) => {
                if let Some(index) = self.compare.iter().position(|c| *c == contender) {
                    self.compare.remove(index);
                } else if self.compare.len() + 1 >= COMPARE_LIMIT {
                    self.error = Some(format!("{COMPARE_LIMIT} agents at most."));
                    return;
                } else {
                    self.compare.push(contender);
                    if matches!(self.workspace, WorkspaceSel::Checkout(_)) {
                        self.workspace = WorkspaceSel::New;
                    }
                }
            }
            Pick::Preset(name) => {
                let Some(preset) = ctx.presets.iter().find(|p| p.name == name).cloned() else {
                    return;
                };
                let installed = self.machine.as_deref().map(|m| ctx.installed(m)).unwrap_or_default();
                if !installed.contains(&preset.harness) {
                    self.error =
                        Some(format!("{} is not installed on this machine.", harness_label_for(&preset.harness)));
                    return;
                }
                self.harness = Some(preset.harness.clone());
                self.model = Some(preset.model.clone());
                self.thinking = Some(preset.thinking.clone()).filter(|t| !t.is_empty());
                self.settle(ctx);
            }
            // Saving and renaming change the app's preset list; the app does it.
            Pick::SavePreset(_) | Pick::RenamePreset { .. } => {}
        }
        self.error = None;
    }

    /// The preset matching the current harness, model and thinking.
    pub fn matching_preset<'p>(&self, presets: &'p [Preset]) -> Option<&'p Preset> {
        let harness = self.harness.as_deref()?;
        presets.iter().find(|p| p.matches(harness, self.model.as_deref(), self.thinking.as_deref()))
    }

    /// The name offered when saving the current choices as a preset.
    pub fn suggested_preset_name(&self, ctx: &Context) -> Option<String> {
        self.model.as_ref()?;
        Some(presets::suggested_name(
            &self.value(ctx, Field::Harness),
            &self.value(ctx, Field::Model),
            self.thinking.as_deref(),
        ))
    }

    /// Steps through past tasks: `older` toward the oldest. Leaving the
    /// history restores the task being written.
    pub fn browse_history(&mut self, history: &[String], older: bool) {
        let next = match (self.history_index, older) {
            (None, true) if !history.is_empty() => {
                self.history_draft = self.task_text();
                Some(0)
            }
            (None, _) => return,
            (Some(index), true) => Some((index + 1).min(history.len().saturating_sub(1))),
            (Some(0), false) => None,
            (Some(index), false) => Some(index - 1),
        };
        self.history_index = next;
        match next {
            Some(index) => self.set_task(&history[index]),
            None => {
                let draft = std::mem::take(&mut self.history_draft);
                self.set_task(&draft);
            }
        }
    }

    /// The task as it is sent: pasted images as their paths.
    pub fn task_text(&self) -> String {
        crate::images::expand(self.task.text(), &self.images)
    }

    /// Fills the task from sent text, its images as `[Image #N]` again.
    pub fn set_task(&mut self, text: &str) {
        let (text, images) = crate::images::collapse(text);
        self.task.set(&text);
        self.images = images;
    }

    /// Whether the task goes to more than one agent.
    pub fn is_comparing(&self) -> bool {
        !self.compare.is_empty()
    }

    /// Every agent the task goes to, the composer's own choice first.
    pub fn contenders(&self) -> Vec<Contender> {
        let Some(harness) = self.harness.clone() else {
            return Vec::new();
        };
        let mut all = vec![Contender { harness, model: self.model.clone(), thinking: self.thinking.clone() }];
        all.extend(self.compare.iter().cloned());
        all
    }

    /// The stem of the branches new worktrees would get: the typed name, or
    /// the prefix and the task's words. None for a checkout, or before any
    /// task is written.
    fn branch_base(&self, ctx: &Context) -> Option<String> {
        match &self.workspace {
            WorkspaceSel::Named(name) => Some(name.clone()),
            // No task, no name worth showing yet.
            WorkspaceSel::New if self.task.is_blank() => None,
            WorkspaceSel::New => {
                let machine = ctx.machine(self.machine.as_deref()?)?;
                let prefix = ctx.config.for_machine(&machine.id, &machine.label, machine.is_local()).branch_prefix;
                Some(format!("{prefix}{}", launch::branch_slug(&launch::task_words(self.task.text()))))
            }
            WorkspaceSel::Checkout(_) => None,
        }
    }

    /// The branches new worktrees would get right now, one per agent. The
    /// name of a comparison carries each agent's harness and model.
    pub fn planned_branches(&self, ctx: &Context) -> Option<Vec<String>> {
        let base = self.branch_base(ctx)?;
        let mut taken: Vec<String> =
            ctx.project(self).map(|p| p.taken_branches().iter().map(|b| b.to_string()).collect()).unwrap_or_default();
        if !self.is_comparing() {
            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
            return Some(vec![match &self.workspace {
                WorkspaceSel::Named(name) => name.clone(),
                _ => launch::unique_branch(&base, &taken),
            }]);
        }
        let mut branches = Vec::new();
        for agent in self.contenders() {
            let wanted = format!("{base}-{}", launch::agent_slug(&agent.harness, agent.model.as_deref()));
            let free: Vec<&str> = taken.iter().map(String::as_str).collect();
            let branch = launch::unique_branch(&wanted, &free);
            taken.push(branch.clone());
            branches.push(branch);
        }
        Some(branches)
    }

    /// The branch a new worktree would get right now. A comparison's
    /// branches share their stem and read as `fix-login-{claude,codex}`.
    pub fn branch_preview(&self, ctx: &Context) -> Option<String> {
        let branches = self.planned_branches(ctx)?;
        if branches.len() < 2 {
            return branches.into_iter().next();
        }
        let base = self.branch_base(ctx)?;
        let stem = format!("{base}-");
        let suffixes: Vec<&str> = branches.iter().map(|b| b.strip_prefix(&stem).unwrap_or(b)).collect();
        Some(format!("{base}-{{{}}}", suffixes.join(",")))
    }

    /// How an agent reads in the Compare row and list: `Codex GPT-5`.
    pub fn contender_label(&self, ctx: &Context, contender: &Contender) -> String {
        let harness = harness_label_for(&contender.harness);
        let catalog = self.machine.as_deref().and_then(|m| ctx.inventory(m)?.models.get(&contender.harness));
        match &contender.model {
            Some(model) => {
                let label = catalog
                    .and_then(|c| c.choices.iter().find(|c| &c.id == model))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| model.clone());
                format!("{harness} {label}")
            }
            None => harness,
        }
    }

    /// Every choice for a field, best match for the picker's query first.
    pub fn choices(&self, ctx: &Context, field: Field, query: &str) -> Vec<Choice> {
        let all = self.all_choices(ctx, field, query);
        crate::fuzzy::rank(query, &all, |c| {
            if let Pick::Model(Some(id)) = &c.pick
                && c.detail == *id
                && let Some(id_score) = crate::fuzzy::score(query, id)
                && crate::fuzzy::score(query, &c.label).is_none_or(|label_score| id_score < label_score)
            {
                return id.clone();
            }
            c.label.clone()
        })
        .into_iter()
        .cloned()
        .collect()
    }

    fn all_choices(&self, ctx: &Context, field: Field, query: &str) -> Vec<Choice> {
        let choice = |label: String, detail: String, pick: Pick| Choice { label, detail, pick, enabled: true };
        match field {
            Field::Task => Vec::new(),
            Field::Preset => self.preset_choices(ctx, query),
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
                        .thinking_levels(self.model.as_deref())
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
                        let detail = if self.is_comparing() {
                            "compared agents each need their own worktree".to_string()
                        } else if checkout.linked {
                            checkout.path.clone()
                        } else {
                            format!("{} · main checkout", checkout.path)
                        };
                        choices.push(Choice {
                            label,
                            detail,
                            pick: Pick::Workspace(WorkspaceSel::Checkout(checkout.path)),
                            enabled: !self.is_comparing(),
                        });
                    }
                }
                choices
            }
            Field::Compare => self.compare_choices(ctx),
        }
    }

    /// Off first, then every preset and every installed harness, each one
    /// ticked when it is in the comparison.
    fn compare_choices(&self, ctx: &Context) -> Vec<Choice> {
        let installed = self.machine.as_deref().map(|m| ctx.installed(m)).unwrap_or_default();
        let full = self.compare.len() + 1 >= COMPARE_LIMIT;
        let mut choices = vec![Choice {
            label: "Off".into(),
            detail: "one agent, the choices above".into(),
            pick: Pick::CompareNone,
            enabled: true,
        }];
        let mut add = |name: String, detail: String, contender: Contender| {
            let added = self.compare.contains(&contender);
            choices.push(Choice {
                label: if added { format!("✓ {name}") } else { name },
                detail: if added {
                    "in the comparison · pick again to drop".into()
                } else if full {
                    format!("{COMPARE_LIMIT} agents at most")
                } else {
                    detail
                },
                pick: Pick::CompareWith(contender),
                enabled: added || !full,
            });
        };
        for preset in ctx.presets.iter().filter(|p| installed.contains(&p.harness)) {
            let thinking = if preset.thinking.is_empty() { String::new() } else { format!(" · {}", preset.thinking) };
            add(
                preset.name.clone(),
                format!("{} · {}{thinking}", harness_label_for(&preset.harness), preset.model),
                Contender {
                    harness: preset.harness.clone(),
                    model: Some(preset.model.clone()),
                    thinking: Some(preset.thinking.clone()).filter(|t| !t.is_empty()),
                },
            );
        }
        for kind in installed {
            add(
                harness_label_for(&kind),
                "its default model".into(),
                Contender { harness: kind, model: None, thinking: None },
            );
        }
        choices
    }

    /// The preset prompts: the strip picks presets, so the popup only ever
    /// asks for a name, to save the current choices or to rename a preset.
    fn preset_choices(&self, ctx: &Context, query: &str) -> Vec<Choice> {
        let typed = query.trim();
        let Some(picker) = self.picker.as_ref() else {
            return Vec::new();
        };
        if let Some(from) = picker.renaming.clone() {
            return vec![Choice {
                label: format!("Rename to {typed}"),
                detail: format!("was {from}"),
                pick: Pick::RenamePreset { from, to: typed.to_string() },
                enabled: !typed.is_empty(),
            }];
        }
        if !picker.saving {
            return Vec::new();
        }
        let ready = self.harness.is_some() && self.model.is_some();
        let taken = ctx.presets.iter().any(|p| p.name == typed);
        // The title says what is saved; the row keeps its room for the name.
        vec![Choice {
            label: format!("Save as {typed}"),
            detail: if !ready {
                "pick a model first".into()
            } else if taken {
                "replaces the existing preset".into()
            } else {
                String::new()
            },
            pick: Pick::SavePreset(typed.to_string()),
            enabled: ready && !typed.is_empty(),
        }]
    }

    /// Every choice for a field, best match first. A preset prompt is its
    /// one action, never ranked.
    pub fn choices_with_actions(&self, ctx: &Context, field: Field, query: &str) -> Vec<Choice> {
        if field == Field::Preset {
            return self.preset_choices(ctx, query);
        }
        self.choices(ctx, field, query)
    }

    /// One launch request per agent, or why nothing can be sent yet. A
    /// comparison is all or nothing: every agent is checked first.
    pub fn requests(&self, ctx: &Context) -> Result<Vec<Request>, String> {
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
        let request = |harness: String, model, thinking, workspace, comparison| Request {
            machine_id: machine_id.clone(),
            machine_label: machine.label.clone(),
            project: project.clone(),
            harness,
            model,
            thinking,
            task: self.task_text(),
            workspace,
            comparison,
        };
        if !self.is_comparing() {
            let workspace = match &self.workspace {
                WorkspaceSel::New => WorkspaceChoice::NewWorktree { branch: None },
                WorkspaceSel::Named(name) => WorkspaceChoice::NewWorktree { branch: Some(name.clone()) },
                WorkspaceSel::Checkout(path) => WorkspaceChoice::Checkout { path: path.clone() },
            };
            return Ok(vec![request(harness, self.model.clone(), self.thinking.clone(), workspace, None)]);
        }
        let installed = ctx.installed(&machine_id);
        if let Some(missing) = self.compare.iter().find(|c| !installed.contains(&c.harness)) {
            return Err(format!("{} is not installed on {}.", harness_label_for(&missing.harness), machine.label));
        }
        let branches = self
            .planned_branches(ctx)
            .ok_or("Compared agents each need their own worktree. Pick New worktree.".to_string())?;
        let contenders = self.contenders();
        let total = contenders.len() as u8;
        Ok(contenders
            .into_iter()
            .zip(branches)
            .enumerate()
            .map(|(index, (agent, branch))| {
                request(
                    agent.harness,
                    agent.model,
                    agent.thinking,
                    WorkspaceChoice::NewWorktree { branch: Some(branch) },
                    Some(Comparison { index: index as u8, total }),
                )
            })
            .collect())
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
            Field::Thinking => {
                if !self.fields(ctx).contains(&Field::Thinking) {
                    "No levels reported".into()
                } else {
                    self.thinking.clone().unwrap_or_else(|| "Default".into())
                }
            }
            Field::Preset => match self.matching_preset(ctx.presets) {
                Some(preset) => preset.name.clone(),
                None => "None".into(),
            },
            Field::Compare => {
                if self.compare.is_empty() {
                    "Off · one agent".into()
                } else {
                    let others: Vec<String> = self.compare.iter().map(|c| self.contender_label(ctx, c)).collect();
                    format!("{} agents · also {}", self.compare.len() + 1, others.join(", "))
                }
            }
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
