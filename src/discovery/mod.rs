//! What can be launched where: each machine's projects (git repositories and
//! their worktrees), installed agent CLIs, and each CLI's models and thinking
//! levels. A small Python probe gathers it on the machine itself.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Effective;

/// The probe, run with `python3 -c` locally or over SSH.
pub const PROBE: &str = include_str!("probe.py");

/// Model catalogs change rarely and cost a few CLI calls each: re-read them
/// at most this often.
pub const MODELS_TTL: Duration = Duration::from_secs(15 * 60);
/// Re-probe old caches once when model capability discovery changes.
pub const MODELS_REVISION: u8 = 1;

/// Herdr's supported agent kinds and the executable each one installs.
pub const HARNESS_EXECUTABLES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("pi", "pi"),
    ("gemini", "gemini"),
    ("copilot", "copilot"),
    ("amp", "amp"),
    ("cursor", "agent"),
    ("grok", "grok"),
    ("kimi", "kimi"),
    ("kiro", "kiro-cli"),
    ("droid", "droid"),
    ("hermes", "hermes"),
    ("kilo", "kilo"),
    ("qwen", "qwen"),
    ("cline", "cline"),
    ("devin", "devin"),
    ("agy", "agy"),
    ("omp", "omp"),
    ("mastracode", "mastracode"),
    ("qodercli", "qodercli"),
    ("letta", "letta"),
    ("maki", "maki"),
    ("muse", "muse"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutEntry {
    pub path: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub checkouts: Vec<CheckoutEntry>,
}

impl Project {
    /// The main checkout first, then linked worktrees.
    pub fn main_checkout(&self) -> CheckoutEntry {
        self.checkouts.iter().find(|c| !c.linked).cloned().unwrap_or_else(|| CheckoutEntry {
            path: self.path.clone(),
            branch: self.branch.clone(),
            linked: false,
        })
    }

    /// Branches that already have a checkout, which a new worktree cannot reuse.
    pub fn taken_branches(&self) -> Vec<&str> {
        self.checkouts.iter().map(|c| c.branch.as_str()).filter(|b| !b.is_empty()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

/// What one installed agent CLI accepts at launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Whether `--model` is accepted.
    #[serde(default)]
    pub selectable: bool,
    #[serde(default)]
    pub session_api: bool,
    #[serde(default)]
    pub default: String,
    /// `--effort`, `--thinking`, or Codex's `--config` override.
    #[serde(default)]
    pub thinking_flag: String,
    #[serde(default)]
    pub thinking: Vec<String>,
    /// Per-model capabilities when a CLI reports them. An empty list means
    /// the model has no reasoning control; absent on older cached inventories.
    #[serde(default)]
    pub thinking_by_model: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    pub fn thinking_levels(&self, model: Option<&str>) -> &[String] {
        let model = model.or_else(|| (!self.default.is_empty()).then_some(self.default.as_str()));
        match model {
            Some(model) if !self.thinking_by_model.is_empty() => {
                self.thinking_by_model.get(model).map(Vec::as_slice).unwrap_or(&[])
            }
            _ => &self.thinking,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub harnesses: Vec<String>,
    #[serde(default)]
    pub models: BTreeMap<String, Catalog>,
    /// Seconds since the epoch when the models were read.
    #[serde(default)]
    pub models_at: u64,
    #[serde(default)]
    pub models_revision: u8,
}

impl Inventory {
    pub fn models_fresh(&self, now: SystemTime) -> bool {
        self.models_revision == MODELS_REVISION
            && !self.models.is_empty()
            && seconds(now).saturating_sub(self.models_at) < MODELS_TTL.as_secs()
    }

    pub fn project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }
}

pub fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The probe's one JSON argument for a machine.
pub fn settings(effective: &Effective, include_models: bool) -> String {
    let mut executables: BTreeMap<String, String> =
        HARNESS_EXECUTABLES.iter().map(|(kind, exe)| (kind.to_string(), exe.to_string())).collect();
    executables.extend(effective.harness_executables.clone());
    let projects: Vec<_> = effective.projects.iter().map(|p| json!({"path": p.path, "name": p.name})).collect();
    json!({
        "roots": effective.roots,
        "depth": effective.depth,
        "projects": projects,
        "executables": executables,
        "include_models": include_models,
    })
    .to_string()
}

/// Turns the probe's output into an inventory. A pass without models keeps
/// the cached ones; configured extra models are added to catalogs that exist.
pub fn finish(
    output: &str,
    cached: Option<&Inventory>,
    include_models: bool,
    configured_models: &BTreeMap<String, Vec<String>>,
    now: SystemTime,
) -> Result<Inventory, String> {
    let json =
        output.lines().rev().find(|line| line.trim_start().starts_with('{')).ok_or("the probe printed nothing")?;
    let mut inventory: Inventory =
        serde_json::from_str(json).map_err(|err| format!("unreadable probe output: {err}"))?;
    if include_models {
        inventory.models_at = seconds(now);
        inventory.models_revision = MODELS_REVISION;
    } else if let Some(cached) = cached {
        inventory.models = cached.models.clone();
        inventory.models_at = cached.models_at;
        inventory.models_revision = cached.models_revision;
    }
    for (harness, extra) in configured_models {
        if let Some(catalog) = inventory.models.get_mut(harness) {
            for id in extra {
                if !catalog.choices.iter().any(|c| &c.id == id) {
                    catalog.choices.push(Choice { id: id.clone(), label: id.clone() });
                }
            }
        }
    }
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProjectEntry};

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn settings_carry_roots_projects_and_every_executable() {
        let mut effective = Config::default().for_machine("local", "Local", true);
        effective.harness_executables.insert("claude".into(), "claude-dev".into());
        effective.projects.push(ProjectEntry { path: "/srv/x".into(), name: Some("x".into()), machine: None });
        let value: serde_json::Value = serde_json::from_str(&settings(&effective, false)).unwrap();
        assert_eq!(value["roots"][0], "~/Work");
        assert_eq!(value["depth"], 2);
        assert_eq!(value["include_models"], false);
        assert_eq!(value["executables"]["cursor"], "agent");
        assert_eq!(value["executables"]["claude"], "claude-dev", "configuration overrides the default");
        assert_eq!(value["executables"].as_object().unwrap().len(), HARNESS_EXECUTABLES.len());
        assert_eq!(value["projects"][0], json!({"path": "/srv/x", "name": "x"}));
    }

    #[test]
    fn the_probe_output_is_parsed_after_any_noise() {
        let output = "Last login: today\n{\"projects\": [{\"name\": \"api\", \"path\": \"/w/api\", \"branch\": \"main\", \"checkouts\": [{\"path\": \"/w/api\", \"branch\": \"main\", \"linked\": false}]}], \"harnesses\": [\"claude\"], \"models\": {\"claude\": {\"choices\": [{\"id\": \"opus\", \"label\": \"Opus\"}], \"selectable\": true, \"thinking_flag\": \"--effort\", \"thinking\": [\"high\"]}}}\n";
        let inventory = finish(output, None, true, &BTreeMap::new(), at(100)).unwrap();
        assert_eq!(inventory.projects[0].name, "api");
        assert_eq!(inventory.harnesses, ["claude"]);
        assert_eq!(inventory.models["claude"].thinking, ["high"]);
        assert_eq!(inventory.models_at, 100);
        assert!(finish("nothing useful", None, true, &BTreeMap::new(), at(1)).is_err());
        assert!(finish("{broken", None, true, &BTreeMap::new(), at(1)).is_err());
    }

    #[test]
    fn a_pass_without_models_keeps_the_cached_ones() {
        let cached = Inventory {
            models: BTreeMap::from([("codex".into(), Catalog { selectable: true, ..Catalog::default() })]),
            models_at: 50,
            ..Inventory::default()
        };
        let fresh = finish(
            r#"{"projects": [], "harnesses": ["codex"], "models": {}}"#,
            Some(&cached),
            false,
            &BTreeMap::new(),
            at(99),
        )
        .unwrap();
        assert!(fresh.models["codex"].selectable);
        assert_eq!(fresh.models_at, 50);
    }

    #[test]
    fn configured_models_extend_existing_catalogs_only() {
        let configured = BTreeMap::from([
            ("claude".to_string(), vec!["opus".to_string(), "claude-opus-5-5".to_string()]),
            ("gemini".to_string(), vec!["gemini-3".to_string()]),
        ]);
        let output = r#"{"projects": [], "harnesses": ["claude"], "models": {"claude": {"choices": [{"id": "opus", "label": "Opus"}]}}}"#;
        let inventory = finish(output, None, true, &configured, at(1)).unwrap();
        let ids: Vec<&str> = inventory.models["claude"].choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["opus", "claude-opus-5-5"], "no duplicate, the new one appended");
        assert!(!inventory.models.contains_key("gemini"), "no catalog for a CLI that is not installed");
    }

    #[cfg(unix)]
    #[test]
    fn regression_codex_catalog_offers_the_cached_reasoning_levels() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("codex");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' '--model <MODEL>' '-c, --config <key=value>'\n")
            .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.path().join("custom-codex");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(
            home.join("models_cache.json"),
            json!({"models": [
                {"slug": "gpt-test", "display_name": "GPT test", "visibility": "list",
                 "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}, {"effort": "ultra"}]},
                {"slug": "gpt-lite", "visibility": "list", "supported_reasoning_levels": [{"effort": "low"}]},
                {"slug": "no-reasoning", "visibility": "list", "supported_reasoning_levels": []},
                {"slug": "hidden", "visibility": "hide", "supported_reasoning_levels": [{"effort": "extreme"}]},
                {"slug": null, "visibility": "list"}
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            home.join("config.toml"),
            "features = []\nmodel = \"gpt-test\"\n[profiles.other]\nmodel = \"gpt-lite\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("python3")
            .arg("-c").arg(PROBE)
            .arg(json!({"roots": [], "depth": 0, "projects": [], "executables": {"codex": executable}, "include_models": true}).to_string())
            .env("HOME", dir.path()).env("CODEX_HOME", &home)
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let inventory = finish(&String::from_utf8_lossy(&output.stdout), None, true, &BTreeMap::new(), at(1)).unwrap();
        let catalog = &inventory.models["codex"];
        assert_eq!(
            catalog.thinking,
            ["low", "high", "ultra"],
            "F8 must offer Codex's advertised levels, including new ones"
        );
        assert_eq!(catalog.thinking_flag, "--config");
        assert_eq!(catalog.default, "gpt-test");
        assert_eq!(catalog.thinking_levels(None), ["low", "high", "ultra"]);
        assert_eq!(catalog.thinking_levels(Some("gpt-lite")), ["low"]);
        assert!(catalog.thinking_levels(Some("no-reasoning")).is_empty());
        assert!(catalog.thinking_levels(Some("unknown-custom-model")).is_empty());
        assert_eq!(catalog.choices.len(), 3);
    }

    #[test]
    fn older_catalogs_still_use_harness_wide_thinking_levels() {
        let catalog: Catalog =
            serde_json::from_str(r#"{"thinking_flag":"--effort","thinking":["high","max"]}"#).unwrap();
        assert_eq!(catalog.thinking_levels(Some("custom-model")), ["high", "max"]);
    }

    #[test]
    fn models_expire_after_fifteen_minutes() {
        let inventory = Inventory {
            models: BTreeMap::from([("pi".into(), Catalog::default())]),
            models_at: 1000,
            models_revision: MODELS_REVISION,
            ..Inventory::default()
        };
        assert!(inventory.models_fresh(at(1000 + 899)));
        assert!(!inventory.models_fresh(at(1000 + 900)));
        assert!(!Inventory::default().models_fresh(at(0)), "no models is never fresh");
        let old = Inventory { models_revision: 0, ..inventory };
        assert!(!old.models_fresh(at(1000)), "old caches must gain Codex reasoning capabilities immediately");
    }

    #[test]
    fn projects_know_their_main_checkout_and_taken_branches() {
        let project = Project {
            name: "cockpit".into(),
            path: "/w/cockpit".into(),
            branch: "main".into(),
            checkouts: vec![
                CheckoutEntry { path: "/h/wt/fix".into(), branch: "fix".into(), linked: true },
                CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false },
            ],
        };
        assert_eq!(project.main_checkout().path, "/w/cockpit");
        assert_eq!(project.taken_branches(), ["fix", "main"]);
        let bare = Project { name: "x".into(), path: "/x".into(), branch: String::new(), checkouts: vec![] };
        assert_eq!(bare.main_checkout().path, "/x");
    }

    #[test]
    fn the_real_probe_finds_repositories_worktrees_and_markers() {
        // Runs the embedded probe with the local python3 over a scratch tree.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = root.join("work/cockpit");
        std::fs::create_dir_all(repo.join(".git/worktrees/fix")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let linked = root.join("elsewhere/fix");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(repo.join(".git/worktrees/fix/HEAD"), "ref: refs/heads/fix-login\n").unwrap();
        std::fs::write(repo.join(".git/worktrees/fix/gitdir"), format!("{}\n", linked.join(".git").display())).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", repo.join(".git/worktrees/fix").display()))
            .unwrap();
        std::fs::create_dir_all(root.join("work/site")).unwrap();
        std::fs::write(root.join("work/site/package.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("work/node_modules/dep/.git")).unwrap();
        let settings = json!({
            "roots": [root.join("work").display().to_string()],
            "depth": 2,
            "projects": [],
            "executables": {"sh": "sh", "nope": "definitely-not-installed-xyz"},
            "include_models": false,
        });
        let output =
            std::process::Command::new("python3").arg("-c").arg(PROBE).arg(settings.to_string()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let inventory = finish(&String::from_utf8_lossy(&output.stdout), None, false, &BTreeMap::new(), at(1)).unwrap();
        let names: Vec<&str> = inventory.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cockpit", "site"], "node_modules is skipped, a manifest makes a project");
        let cockpit = inventory.project("cockpit").unwrap();
        assert_eq!(cockpit.branch, "main");
        let fix = cockpit.checkouts.iter().find(|c| c.linked).expect("the linked worktree is grouped under its repo");
        assert_eq!(fix.branch, "fix-login");
        assert_eq!(inventory.harnesses, ["sh"], "only installed executables count");
    }
}
