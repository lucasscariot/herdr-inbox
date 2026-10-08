//! Presets: named combinations of harness, model and thinking level, picked
//! in one move in the composer. Stored as `presets.json` in the legacy
//! plugin's format.

use serde::{Deserialize, Serialize};

pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub harness: String,
    pub model: String,
    #[serde(default)]
    pub thinking: String,
}

impl Preset {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("A preset needs a name.".into());
        }
        if self.harness.trim().is_empty() || self.model.trim().is_empty() {
            return Err("A preset needs a harness and a model. Pick a model first.".into());
        }
        if !self.thinking.is_empty() && !THINKING_LEVELS.contains(&self.thinking.as_str()) {
            return Err(format!("Unknown thinking level {}", self.thinking));
        }
        Ok(())
    }

    /// Whether the composer's current choices are exactly this preset.
    pub fn matches(&self, harness: &str, model: Option<&str>, thinking: Option<&str>) -> bool {
        self.harness == harness && Some(self.model.as_str()) == model && self.thinking == thinking.unwrap_or("")
    }
}

/// Keeps only valid presets with unique names, in order; a broken file must
/// not lose the good ones.
pub fn clean(presets: Vec<Preset>) -> Vec<Preset> {
    let mut seen = std::collections::HashSet::new();
    presets
        .into_iter()
        .map(|p| Preset {
            name: p.name.trim().to_string(),
            harness: p.harness.trim().to_string(),
            model: p.model.trim().to_string(),
            thinking: p.thinking.trim().to_string(),
        })
        .filter(|p| p.validate().is_ok() && seen.insert(p.name.clone()))
        .collect()
}

/// Adds a preset, replacing one with the same name.
pub fn save(presets: &[Preset], preset: Preset) -> Result<Vec<Preset>, String> {
    preset.validate()?;
    let mut next: Vec<Preset> = presets.iter().filter(|p| p.name != preset.name).cloned().collect();
    next.push(preset);
    Ok(next)
}

/// Renames a preset; the new name must be free.
pub fn rename(presets: &[Preset], from: &str, to: &str) -> Result<Vec<Preset>, String> {
    let to = to.trim();
    if to.is_empty() {
        return Err("A preset needs a name.".into());
    }
    if to != from && presets.iter().any(|p| p.name == to) {
        return Err("A preset already has this name. Choose another name.".into());
    }
    if !presets.iter().any(|p| p.name == from) {
        return Err(format!("No preset named {from}"));
    }
    Ok(presets
        .iter()
        .map(|p| if p.name == from { Preset { name: to.into(), ..p.clone() } } else { p.clone() })
        .collect())
}

pub fn remove(presets: &[Preset], name: &str) -> Vec<Preset> {
    presets.iter().filter(|p| p.name != name).cloned().collect()
}

/// A readable default name: "Claude · Opus · High".
pub fn suggested_name(harness_label: &str, model_label: &str, thinking: Option<&str>) -> String {
    let mut parts = vec![harness_label.to_string(), model_label.to_string()];
    if let Some(level) = thinking.filter(|t| !t.is_empty()) {
        let mut chars = level.chars();
        if let Some(first) = chars.next() {
            parts.push(first.to_uppercase().chain(chars).collect());
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, harness: &str, model: &str, thinking: &str) -> Preset {
        Preset { name: name.into(), harness: harness.into(), model: model.into(), thinking: thinking.into() }
    }

    #[test]
    fn the_legacy_file_reads_as_is() {
        let json = r#"[{"name": "Pi · GPT-6.1 Sol · Max", "harness": "pi", "model": "openai-codex/gpt-6.1-sol", "thinking": "max"},
                       {"name": "Claude · Opus", "harness": "claude", "model": "opus", "thinking": ""},
                       {"name": "No thinking key", "harness": "codex", "model": "gpt-5"}]"#;
        let presets: Vec<Preset> = serde_json::from_str(json).unwrap();
        assert_eq!(clean(presets.clone()).len(), 3);
        assert_eq!(presets[2].thinking, "");
    }

    #[test]
    fn clean_drops_invalid_and_duplicate_presets_but_keeps_the_rest() {
        let presets = vec![
            preset("ok", "claude", "opus", ""),
            preset("  ", "claude", "opus", ""),
            preset("no model", "claude", " ", ""),
            preset("bad thinking", "claude", "opus", "ultra"),
            preset("ok", "codex", "gpt", ""),
            preset(" spaced ", " pi ", " m ", " high "),
        ];
        let cleaned = clean(presets);
        assert_eq!(cleaned, vec![preset("ok", "claude", "opus", ""), preset("spaced", "pi", "m", "high")]);
    }

    #[test]
    fn saving_replaces_a_preset_with_the_same_name() {
        let presets = vec![preset("a", "claude", "opus", ""), preset("b", "pi", "x", "")];
        let next = save(&presets, preset("a", "claude", "sonnet", "high")).unwrap();
        assert_eq!(next, vec![preset("b", "pi", "x", ""), preset("a", "claude", "sonnet", "high")]);
        assert!(save(&presets, preset("c", "claude", "", "")).unwrap_err().contains("Pick a model"));
    }

    #[test]
    fn renaming_refuses_taken_and_empty_names() {
        let presets = vec![preset("a", "claude", "opus", ""), preset("b", "pi", "x", "")];
        assert_eq!(rename(&presets, "a", "z").unwrap()[0].name, "z");
        assert_eq!(rename(&presets, "a", "a").unwrap(), presets, "renaming to itself is fine");
        assert!(rename(&presets, "a", "b").unwrap_err().contains("already has this name"));
        assert!(rename(&presets, "a", " ").is_err());
        assert!(rename(&presets, "missing", "x").is_err());
    }

    #[test]
    fn removing_and_matching() {
        let presets = vec![preset("a", "claude", "opus", "high"), preset("b", "pi", "x", "")];
        assert_eq!(remove(&presets, "a"), vec![preset("b", "pi", "x", "")]);
        assert!(presets[0].matches("claude", Some("opus"), Some("high")));
        assert!(!presets[0].matches("claude", Some("opus"), None));
        assert!(presets[1].matches("pi", Some("x"), None));
        assert!(presets[1].matches("pi", Some("x"), Some("")));
        assert!(!presets[1].matches("pi", None, None), "a preset always names a model");
    }

    #[test]
    fn suggested_names_read_naturally() {
        assert_eq!(suggested_name("Claude", "Opus", Some("high")), "Claude · Opus · High");
        assert_eq!(suggested_name("Codex", "gpt-5", None), "Codex · gpt-5");
        assert_eq!(suggested_name("Codex", "gpt-5", Some("")), "Codex · gpt-5");
    }
}
