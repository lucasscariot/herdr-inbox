//! The machines the inbox shows: the local one, plus every enabled SSH
//! machine saved in Herdr (`herdr machine list --json`).

use serde::Deserialize;

use crate::app::MachineInfo;
use crate::herdr::ssh::SshHerdr;
use crate::herdr::transport::Runner;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Profile {
    id: String,
    #[serde(default)]
    label: String,
    target: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// A saved machine, ready to connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub info: MachineInfo,
    pub ssh: SshHerdr,
}

/// Parses `herdr machine list --json`. Disabled machines are left out, and an
/// unreadable list means no saved machines rather than an error: the local
/// machine always works.
pub fn parse(json: &str) -> Vec<Remote> {
    let profiles: Vec<Profile> = match serde_json::from_str(json) {
        Ok(profiles) => profiles,
        Err(_) => return Vec::new(),
    };
    profiles
        .into_iter()
        .filter(|p| p.enabled && !p.id.is_empty() && !p.target.is_empty())
        .map(|p| {
            let session = p.session.filter(|s| !s.is_empty() && s != "default");
            let label = if p.label.trim().is_empty() { p.target.clone() } else { p.label.trim().to_string() };
            Remote { info: MachineInfo { id: p.id, label }, ssh: SshHerdr::new(p.target, session) }
        })
        .collect()
}

/// Asks the local `herdr` for its saved machines.
pub fn load(local: &Runner) -> Vec<Remote> {
    match local.command(&["machine", "list", "--json"]).output() {
        Ok(output) if output.status.success() => parse(&String::from_utf8_lossy(&output.stdout)),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_machines_are_loaded_with_their_session() {
        let json = r#"[
            {"id": "b6f5", "label": "Mac Studio", "target": "lucas@mac-studio", "session": "default", "enabled": true, "selected": false},
            {"id": "ec07", "label": "MacBook", "target": "lucasscariot@macbook", "session": "night", "enabled": true},
            {"id": "off1", "label": "Old", "target": "old@box", "enabled": false}
        ]"#;
        let remotes = parse(json);
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].info, MachineInfo { id: "b6f5".into(), label: "Mac Studio".into() });
        assert_eq!(remotes[0].ssh.target, "lucas@mac-studio");
        assert_eq!(remotes[0].ssh.session, None, "the default session needs no flag");
        assert_eq!(remotes[1].ssh.session.as_deref(), Some("night"));
    }

    #[test]
    fn a_missing_label_falls_back_to_the_target() {
        let remotes = parse(r#"[{"id": "x", "label": " ", "target": "me@box"}]"#);
        assert_eq!(remotes[0].info.label, "me@box");
    }

    #[test]
    fn broken_or_incomplete_lists_yield_no_machines() {
        assert!(parse("not json").is_empty());
        assert!(parse("{}").is_empty());
        assert!(parse(r#"[{"id": "", "target": "a@b"}, {"id": "x", "target": ""}]"#).is_empty());
    }

    #[test]
    fn a_failing_herdr_yields_no_machines() {
        let runner = Runner::Local(crate::herdr::terminal::HerdrCommand::new("/nonexistent/herdr"));
        assert!(load(&runner).is_empty());
    }
}
