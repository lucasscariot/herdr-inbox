//! User configuration: `$XDG_CONFIG_HOME/herdr-inbox/config.toml`.
//!
//! Until that file exists, the legacy plugin's `config.json` is read instead,
//! so a user coming from the plugin keeps their roots and machine overrides.
//! Every key is optional.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_ROOTS: [&str; 3] = ["~/Work", "~/Projects", "~/goinfre"];
pub const DEFAULT_DEPTH: u8 = 2;
pub const MAX_DEPTH: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// Run in the project's checkout.
    Checkout,
    /// Create a git worktree with a branch named after the task. The default:
    /// many agents on one repository must not touch each other's files.
    #[default]
    Worktree,
}

/// A repository listed by hand, for projects outside the scanned roots.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProjectEntry {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Machine id or label; the local machine when absent.
    #[serde(default)]
    pub machine: Option<String>,
}

/// Settings that one machine may override. An override replaces the global
/// value entirely; lists and maps are not merged.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct MachineSettings {
    pub roots: Option<Vec<String>>,
    pub depth: Option<u8>,
    pub branch_prefix: Option<String>,
    pub harness_args: Option<BTreeMap<String, Vec<String>>>,
    pub harness_executables: Option<BTreeMap<String, String>>,
    pub models: Option<BTreeMap<String, Vec<String>>>,
    /// How long `herdr agent start` waits for an agent to be ready.
    pub agent_start_timeout_ms: Option<u64>,
}

/// Herdr's default for an agent to come up, and its accepted range.
pub const AGENT_START_TIMEOUT_MS: u64 = 45_000;
const AGENT_START_RANGE: std::ops::RangeInclusive<u64> = 3_001..=300_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Config {
    #[serde(flatten)]
    pub global: MachineSettings,
    #[serde(default)]
    pub default_workspace: WorkspaceMode,
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
    /// Overrides by machine id or label.
    #[serde(default)]
    pub machines: BTreeMap<String, MachineSettings>,
}

/// The effective settings for one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub roots: Vec<String>,
    pub depth: u8,
    pub branch_prefix: String,
    pub harness_args: BTreeMap<String, Vec<String>>,
    pub harness_executables: BTreeMap<String, String>,
    pub models: BTreeMap<String, Vec<String>>,
    pub agent_start_timeout_ms: u64,
    pub projects: Vec<ProjectEntry>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
}

impl Config {
    pub fn parse_toml(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|err| invalid(path, err.message()))?;
        config.validate(path)
    }

    pub fn parse_json(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let config: Config = serde_json::from_str(text).map_err(|err| invalid(path, &err.to_string()))?;
        config.validate(path)
    }

    fn validate(self, path: &Path) -> Result<Self, ConfigError> {
        let depths = std::iter::once(self.global.depth).chain(self.machines.values().map(|m| m.depth));
        if depths.flatten().any(|depth| depth > MAX_DEPTH) {
            return Err(invalid(path, &format!("depth must be between 0 and {MAX_DEPTH}")));
        }
        let executables = std::iter::once(&self.global.harness_executables)
            .chain(self.machines.values().map(|m| &m.harness_executables))
            .flatten();
        let timeouts = std::iter::once(self.global.agent_start_timeout_ms)
            .chain(self.machines.values().map(|m| m.agent_start_timeout_ms))
            .flatten();
        if timeouts.into_iter().any(|t| !AGENT_START_RANGE.contains(&t)) {
            return Err(invalid(path, "agent_start_timeout_ms must be between 3001 and 300000"));
        }
        if executables.flat_map(|map| map.values()).any(|exe| exe.trim().is_empty()) {
            return Err(invalid(path, "harness_executables cannot map a harness to an empty name"));
        }
        Ok(self)
    }

    /// The settings for a machine, looked up by id, then by label.
    pub fn for_machine(&self, id: &str, label: &str, local: bool) -> Effective {
        let own = self.machines.get(id).or_else(|| self.machines.get(label));
        let global = &self.global;
        Effective {
            roots: pick(own, global, |m| m.roots.clone())
                .unwrap_or_else(|| DEFAULT_ROOTS.iter().map(|r| r.to_string()).collect()),
            depth: pick(own, global, |m| m.depth).unwrap_or(DEFAULT_DEPTH),
            branch_prefix: pick(own, global, |m| m.branch_prefix.clone()).unwrap_or_default(),
            harness_args: pick(own, global, |m| m.harness_args.clone()).unwrap_or_default(),
            harness_executables: pick(own, global, |m| m.harness_executables.clone()).unwrap_or_default(),
            models: pick(own, global, |m| m.models.clone()).unwrap_or_default(),
            agent_start_timeout_ms: pick(own, global, |m| m.agent_start_timeout_ms).unwrap_or(AGENT_START_TIMEOUT_MS),
            projects: self
                .projects
                .iter()
                .filter(|p| match p.machine.as_deref() {
                    None | Some("local") | Some("Local") => local,
                    Some(machine) => machine == id || machine == label,
                })
                .cloned()
                .collect(),
        }
    }
}

/// A machine's own value, else the global one.
fn pick<T>(
    own: Option<&MachineSettings>,
    global: &MachineSettings,
    field: impl Fn(&MachineSettings) -> Option<T>,
) -> Option<T> {
    own.and_then(&field).or_else(|| field(global))
}

fn invalid(path: &Path, message: &str) -> ConfigError {
    ConfigError::Invalid { path: path.display().to_string(), message: message.to_string() }
}

/// Where the config lives and where the plugin kept its own.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub legacy: PathBuf,
}

impl Paths {
    pub fn from_env(env: &crate::herdr::socket::SocketEnv) -> Option<Self> {
        let config_home = env.xdg_config_home.clone().or_else(|| env.home.as_ref().map(|h| h.join(".config")))?;
        Some(Self {
            config: config_home.join("herdr-inbox/config.toml"),
            legacy: config_home.join("herdr/plugins/config/lucasscariot.herdr-inbox/config.json"),
        })
    }
}

/// Loads `config.toml`, else the legacy `config.json`, else the defaults.
pub fn load(paths: &Paths) -> Result<Config, ConfigError> {
    match std::fs::read_to_string(&paths.config) {
        Ok(text) => return Config::parse_toml(&text, &paths.config),
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            return Err(invalid(&paths.config, &err.to_string()));
        }
        Err(_) => {}
    }
    match std::fs::read_to_string(&paths.legacy) {
        Ok(text) => Config::parse_json(&text, &paths.legacy),
        Err(_) => Ok(Config::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml(text: &str) -> Config {
        Config::parse_toml(text, Path::new("config.toml")).unwrap()
    }

    #[test]
    fn an_empty_config_uses_the_defaults() {
        let effective = Config::default().for_machine("local", "Local", true);
        assert_eq!(effective.roots, ["~/Work", "~/Projects", "~/goinfre"]);
        assert_eq!(effective.depth, 2);
        assert_eq!(effective.branch_prefix, "");
        assert!(effective.harness_args.is_empty());
        assert_eq!(Config::default().default_workspace, WorkspaceMode::Worktree);
    }

    #[test]
    fn global_settings_apply_to_every_machine() {
        let config = toml(
            "roots = [\"~/code\"]\ndepth = 1\nbranch_prefix = \"lucas/\"\ndefault_workspace = \"checkout\"\n[harness_args]\nclaude = [\"--verbose\"]\n",
        );
        let effective = config.for_machine("abc", "Studio", false);
        assert_eq!(effective.roots, ["~/code"]);
        assert_eq!(effective.depth, 1);
        assert_eq!(effective.branch_prefix, "lucas/");
        assert_eq!(effective.harness_args["claude"], ["--verbose"]);
        assert_eq!(config.default_workspace, WorkspaceMode::Checkout);
    }

    #[test]
    fn a_machine_override_replaces_rather_than_merges() {
        let config = toml(
            "[harness_args]\nclaude = [\"--verbose\"]\ncodex = [\"--a\"]\n\n[machines.\"Mac Studio\"]\nharness_args = { codex = [\"--no-daemon\"] }\ndepth = 3\n",
        );
        let studio = config.for_machine("b6f5", "Mac Studio", false);
        assert_eq!(studio.harness_args.len(), 1, "the machine's map replaces the global one");
        assert_eq!(studio.harness_args["codex"], ["--no-daemon"]);
        assert_eq!(studio.depth, 3);
        assert_eq!(studio.roots, DEFAULT_ROOTS.map(String::from), "untouched settings fall through");
        let local = config.for_machine("local", "Local", true);
        assert_eq!(local.harness_args["claude"], ["--verbose"]);
    }

    #[test]
    fn overrides_are_found_by_id_before_label() {
        let config = toml("[machines.b6f5]\ndepth = 1\n[machines.\"Mac Studio\"]\ndepth = 3\n");
        assert_eq!(config.for_machine("b6f5", "Mac Studio", false).depth, 1);
        assert_eq!(config.for_machine("other", "Mac Studio", false).depth, 3);
    }

    #[test]
    fn listed_projects_go_to_their_machine() {
        let config = toml(
            "[[projects]]\npath = \"/srv/a\"\n[[projects]]\npath = \"/srv/b\"\nmachine = \"Mac Studio\"\nname = \"bee\"\n[[projects]]\npath = \"/srv/c\"\nmachine = \"local\"\n",
        );
        let local: Vec<String> =
            config.for_machine("local", "Local", true).projects.into_iter().map(|p| p.path).collect();
        assert_eq!(local, ["/srv/a", "/srv/c"]);
        let studio = config.for_machine("b6f5", "Mac Studio", false).projects;
        assert_eq!(studio.len(), 1);
        assert_eq!(studio[0].name.as_deref(), Some("bee"));
    }

    #[test]
    fn the_legacy_plugin_json_reads_the_same_way() {
        let json = r#"{"roots": ["~/Work"], "depth": 2, "machines": {"Mac Studio": {"harness_args": {"codex": ["--no-daemon"]}}}, "projects": [], "presets": [{"name": "x"}]}"#;
        let config = Config::parse_json(json, Path::new("config.json")).unwrap();
        assert_eq!(config.for_machine("id", "Mac Studio", false).harness_args["codex"], ["--no-daemon"]);
    }

    #[test]
    fn invalid_values_name_the_file() {
        let err = Config::parse_toml("depth = 4", Path::new("/c/config.toml")).unwrap_err();
        assert!(err.to_string().starts_with("/c/config.toml: depth must be between 0 and 3"), "{err}");
        assert!(Config::parse_toml("[machines.x]\ndepth = 9", Path::new("c")).is_err());
        assert!(Config::parse_toml("roots = 3", Path::new("c")).is_err());
        assert!(Config::parse_toml("default_workspace = \"cloud\"", Path::new("c")).is_err());
        assert!(Config::parse_toml("[harness_executables]\nclaude = \" \"", Path::new("c")).is_err());
        assert!(
            Config::parse_toml("agent_start_timeout_ms = 3000", Path::new("c")).is_err(),
            "Herdr wants more than 3 s"
        );
        assert!(Config::parse_toml("agent_start_timeout_ms = 300001", Path::new("c")).is_err());
        let fast = Config::parse_toml("agent_start_timeout_ms = 4000", Path::new("c")).unwrap();
        assert_eq!(fast.for_machine("local", "Local", true).agent_start_timeout_ms, 4000);
        assert_eq!(Config::default().for_machine("local", "Local", true).agent_start_timeout_ms, 45_000);
    }

    #[test]
    fn the_new_file_wins_over_the_legacy_one_and_defaults_fill_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { config: dir.path().join("new/config.toml"), legacy: dir.path().join("old/config.json") };
        assert_eq!(load(&paths).unwrap(), Config::default());
        std::fs::create_dir_all(paths.legacy.parent().unwrap()).unwrap();
        std::fs::write(&paths.legacy, r#"{"depth": 1}"#).unwrap();
        assert_eq!(load(&paths).unwrap().global.depth, Some(1));
        std::fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        std::fs::write(&paths.config, "depth = 3").unwrap();
        assert_eq!(load(&paths).unwrap().global.depth, Some(3));
    }

    #[test]
    fn paths_follow_xdg_config_home() {
        let env = crate::herdr::socket::SocketEnv { home: Some("/h".into()), ..Default::default() };
        let paths = Paths::from_env(&env).unwrap();
        assert_eq!(paths.config, PathBuf::from("/h/.config/herdr-inbox/config.toml"));
        assert_eq!(paths.legacy, PathBuf::from("/h/.config/herdr/plugins/config/lucasscariot.herdr-inbox/config.json"));
        let env = crate::herdr::socket::SocketEnv { xdg_config_home: Some("/x".into()), ..env };
        assert_eq!(Paths::from_env(&env).unwrap().config, PathBuf::from("/x/herdr-inbox/config.toml"));
    }
}
