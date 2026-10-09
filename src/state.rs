//! What the inbox remembers between runs, in `$XDG_STATE_HOME/herdr-inbox`:
//! composer choices, each machine's discovered projects and harnesses, and
//! one journal per launch. Every write is atomic and private to the user.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// How many past tasks the composer recalls.
pub const HISTORY_LIMIT: usize = 50;

/// Files carried over from the legacy plugin's state, once.
const MIGRATED: [&str; 4] = ["preferences.json", "presets.json", "history.json", "credentials.json"];
const MIGRATION_MARKER: &str = ".migrated-from-plugin";

#[derive(Debug, Clone)]
pub struct State {
    dir: PathBuf,
}

/// Choices remembered for one project. Machine, harness and workspace mode
/// are per project; model and thinking are per project, machine and harness,
/// so switching hosts never carries over another host's model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectChoices {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// machine id → harness → model id
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, BTreeMap<String, String>>,
    /// machine id → harness → thinking level
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub thinking: BTreeMap<String, BTreeMap<String, String>>,
}

/// The legacy plugin's `preferences.json` layout: `last_project` next to one
/// object per project name, so its file can be read as is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_project: Option<String>,
    #[serde(flatten)]
    pub projects: BTreeMap<String, ProjectChoices>,
}

impl Preferences {
    pub fn project(&self, name: &str) -> ProjectChoices {
        self.projects.get(name).cloned().unwrap_or_default()
    }

    /// Records a launch's choices. Empty model or thinking clear the saved one.
    pub fn remember(&mut self, launch: &Remembered) {
        self.last_project = Some(launch.project.clone());
        let entry = self.projects.entry(launch.project.clone()).or_default();
        entry.machine = Some(launch.machine.clone());
        entry.harness = Some(launch.harness.clone());
        entry.workspace = Some(launch.workspace.clone());
        let set = |map: &mut BTreeMap<String, BTreeMap<String, String>>, value: &str| {
            let by_harness = map.entry(launch.machine.clone()).or_default();
            if value.is_empty() {
                by_harness.remove(&launch.harness);
            } else {
                by_harness.insert(launch.harness.clone(), value.to_string());
            }
        };
        set(&mut entry.models, &launch.model);
        set(&mut entry.thinking, &launch.thinking);
    }
}

/// What a successful launch teaches the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    pub project: String,
    pub machine: String,
    pub harness: String,
    pub workspace: String,
    pub model: String,
    pub thinking: String,
}

impl State {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `$XDG_STATE_HOME/herdr-inbox`, else `~/.local/state/herdr-inbox`.
    pub fn default_dir() -> Option<PathBuf> {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        var("XDG_STATE_HOME")
            .or_else(|| var("HOME").map(|home| home.join(".local/state")))
            .map(|base| base.join("herdr-inbox"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads a JSON file, or the default when it is missing or unreadable:
    /// remembered state must never stop the inbox from starting.
    pub fn read<T: DeserializeOwned + Default>(&self, relative: &str) -> T {
        std::fs::read(self.dir.join(relative))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes JSON atomically: a private temporary file, synced, then renamed
    /// over the target, so readers never see half a file.
    pub fn write<T: Serialize>(&self, relative: &str, value: &T) -> std::io::Result<()> {
        let path = self.dir.join(relative);
        let parent = path.parent().unwrap_or(&self.dir);
        std::fs::create_dir_all(parent)?;
        let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let mut temporary = tempfile::Builder::new().prefix(".inbox-").tempfile_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path).map_err(|err| err.error)?;
        Ok(())
    }

    pub fn remove(&self, relative: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.dir.join(relative)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// The names of the JSON files in a subdirectory, without extension.
    pub fn list(&self, relative: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.dir.join(relative)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
            .collect();
        names.sort();
        names
    }

    pub fn preferences(&self) -> Preferences {
        self.read("preferences.json")
    }

    /// Past tasks, newest first.
    pub fn history(&self) -> Vec<String> {
        self.read("history.json")
    }

    /// Adds a task to the top of the history, without duplicates.
    pub fn push_history(&self, task: &str) -> std::io::Result<Vec<String>> {
        let history = with_task(self.history(), task);
        self.write("history.json", &history)?;
        Ok(history)
    }

    pub fn presets(&self) -> Vec<crate::presets::Preset> {
        crate::presets::clean(self.read("presets.json"))
    }

    pub fn save_presets(&self, presets: &[crate::presets::Preset]) -> std::io::Result<()> {
        self.write("presets.json", &presets)
    }

    /// The legacy plugin's state directory: `HERDR_PLUGIN_STATE_DIR`, else
    /// `~/.local/state/herdr/plugins/lucasscariot.herdr-inbox`.
    pub fn legacy_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
        Some(PathBuf::from(home).join(".local/state/herdr/plugins/lucasscariot.herdr-inbox"))
    }

    /// Copies the plugin's preferences, presets, history and credentials the
    /// first time the inbox runs, without overwriting anything. Returns the
    /// files copied.
    pub fn migrate_from(&self, legacy: &Path) -> Vec<&'static str> {
        if self.dir.join(MIGRATION_MARKER).exists() || !legacy.is_dir() {
            return Vec::new();
        }
        let mut copied = Vec::new();
        for name in MIGRATED {
            let source = legacy.join(name);
            if self.dir.join(name).exists() || !source.is_file() {
                continue;
            }
            let value: Option<serde_json::Value> =
                std::fs::read(&source).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
            if let Some(value) = value
                && self.write(name, &value).is_ok()
            {
                copied.push(name);
            }
        }
        let _ = self.write(MIGRATION_MARKER, &copied);
        copied
    }

    /// Re-reads before writing, so two inbox windows merge their choices
    /// instead of overwriting each other's projects.
    pub fn remember(&self, launch: &Remembered) -> std::io::Result<()> {
        let mut preferences = self.preferences();
        preferences.remember(launch);
        self.write("preferences.json", &preferences)
    }
}

/// The history with `task` on top, deduplicated and bounded.
pub fn with_task(mut history: Vec<String>, task: &str) -> Vec<String> {
    let task = task.trim();
    if task.is_empty() {
        return history;
    }
    history.retain(|t| t != task);
    history.insert(0, task.to_string());
    history.truncate(HISTORY_LIMIT);
    history
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remembered(project: &str, machine: &str, harness: &str, model: &str) -> Remembered {
        Remembered {
            project: project.into(),
            machine: machine.into(),
            harness: harness.into(),
            workspace: "worktree".into(),
            model: model.into(),
            thinking: String::new(),
        }
    }

    #[test]
    fn missing_or_corrupt_files_read_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        assert_eq!(state.preferences(), Preferences::default());
        std::fs::write(dir.path().join("preferences.json"), "{ not json").unwrap();
        assert_eq!(state.preferences(), Preferences::default());
    }

    #[test]
    fn writes_are_atomic_and_create_directories() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path().join("deep/state"));
        state.write("inventory/abc.json", &serde_json::json!({"a": 1})).unwrap();
        let read: serde_json::Value = state.read("inventory/abc.json");
        assert_eq!(read["a"], 1);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("deep/state/inventory"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".inbox-"))
            .collect();
        assert!(leftovers.is_empty(), "no temporary file is left behind");
    }

    #[test]
    fn state_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        state.write("credentials.json", &serde_json::json!({})).unwrap();
        let mode = std::fs::metadata(dir.path().join("credentials.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "group and others get nothing: {mode:o}");
    }

    #[test]
    fn listing_and_removing_journals() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        assert!(state.list("launches").is_empty());
        state.write("launches/b.json", &1).unwrap();
        state.write("launches/a.json", &2).unwrap();
        std::fs::write(dir.path().join("launches/notes.txt"), "x").unwrap();
        assert_eq!(state.list("launches"), ["a", "b"]);
        state.remove("launches/a.json").unwrap();
        state.remove("launches/a.json").unwrap();
        assert_eq!(state.list("launches"), ["b"]);
    }

    #[test]
    fn choices_are_remembered_per_project_machine_and_harness() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        state.remember(&remembered("cockpit", "local", "claude", "opus")).unwrap();
        state.remember(&remembered("cockpit", "studio", "claude", "sonnet")).unwrap();
        state.remember(&remembered("api", "local", "codex", "")).unwrap();
        let preferences = state.preferences();
        assert_eq!(preferences.last_project.as_deref(), Some("api"));
        let cockpit = preferences.project("cockpit");
        assert_eq!(cockpit.machine.as_deref(), Some("studio"), "the last machine used");
        assert_eq!(cockpit.models["local"]["claude"], "opus");
        assert_eq!(cockpit.models["studio"]["claude"], "sonnet", "another host keeps its own model");
        assert!(preferences.project("api").models.get("local").is_none_or(|m| m.is_empty()));
    }

    #[test]
    fn an_empty_model_clears_the_saved_one() {
        let mut preferences = Preferences::default();
        preferences.remember(&remembered("p", "local", "claude", "opus"));
        preferences.remember(&remembered("p", "local", "claude", ""));
        assert!(preferences.project("p").models["local"].is_empty());
    }

    #[test]
    fn the_legacy_preferences_file_reads_as_is() {
        let legacy = r#"{
            "last_project": "cockpit",
            "cockpit": {"machine": "local", "harness": "claude", "workspace": "worktree",
                        "models": {"local": {"claude": "opus"}}, "thinking": {"local": {"claude": "high"}}}
        }"#;
        let preferences: Preferences = serde_json::from_str(legacy).unwrap();
        assert_eq!(preferences.last_project.as_deref(), Some("cockpit"));
        let cockpit = preferences.project("cockpit");
        assert_eq!(cockpit.workspace.as_deref(), Some("worktree"));
        assert_eq!(cockpit.thinking["local"]["claude"], "high");
        let round_trip: Preferences = serde_json::from_str(&serde_json::to_string(&preferences).unwrap()).unwrap();
        assert_eq!(round_trip, preferences);
    }

    #[test]
    fn history_keeps_the_newest_fifty_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        state.push_history("first").unwrap();
        state.push_history("second").unwrap();
        state.push_history("  first ").unwrap();
        state.push_history("   ").unwrap();
        assert_eq!(state.history(), ["first", "second"]);
        let long: Vec<String> = (0..60).map(|i| i.to_string()).collect();
        let mut history = Vec::new();
        for task in &long {
            history = with_task(history, task);
        }
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[0], "59");
    }

    #[test]
    fn presets_round_trip_and_bad_ones_are_dropped_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        assert!(state.presets().is_empty());
        let good = crate::presets::Preset {
            name: "a".into(),
            harness: "claude".into(),
            model: "opus".into(),
            thinking: String::new(),
        };
        state.save_presets(std::slice::from_ref(&good)).unwrap();
        assert_eq!(state.presets(), vec![good]);
        std::fs::write(dir.path().join("presets.json"), r#"[{"name": "x"}]"#).unwrap();
        assert!(state.presets().is_empty());
    }

    #[test]
    fn the_plugin_state_is_migrated_once_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("history.json"), r#"["old task"]"#).unwrap();
        std::fs::write(legacy.join("presets.json"), r#"[{"name": "p", "harness": "claude", "model": "opus"}]"#)
            .unwrap();
        std::fs::write(legacy.join("credentials.json"), r#"{"backend": "", "keys": {"groq": "gsk"}}"#).unwrap();
        std::fs::write(legacy.join("preferences.json"), "not json").unwrap();
        let state = State::new(dir.path().join("new"));
        state.write("history.json", &vec!["new task"]).unwrap();
        let copied = state.migrate_from(&legacy);
        assert_eq!(copied, ["presets.json", "credentials.json"], "existing and unreadable files are skipped");
        assert_eq!(state.history(), ["new task"]);
        assert_eq!(state.presets().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("new/credentials.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "keys stay private");
        std::fs::remove_file(dir.path().join("new/presets.json")).unwrap();
        assert!(state.migrate_from(&legacy).is_empty(), "only once");
        assert!(state.presets().is_empty());
        assert!(State::new(dir.path().join("other")).migrate_from(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn two_writers_merge_instead_of_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let first = State::new(dir.path());
        let second = State::new(dir.path());
        first.remember(&remembered("a", "local", "claude", "")).unwrap();
        second.remember(&remembered("b", "local", "codex", "")).unwrap();
        let preferences = first.preferences();
        assert!(preferences.projects.contains_key("a") && preferences.projects.contains_key("b"));
    }
}
