//! How much of each subscription is left: the windows behind a harness, read
//! through the CLIs' own channels and never through a provider token.
//!
//! Codex answers its documented app server (`account/rateLimits/read`), with
//! its session log as a fallback. Claude Code hands its numbers to the status
//! line it runs; the inbox gives every Claude thread it launches a wrapper that
//! caches them. A provider is the account whose limits apply: Anthropic behind
//! `claude`, OpenAI behind `codex`; the config can map other harnesses to one.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::json;

/// The probe, run with `python3 -c` locally or over SSH.
pub const PROBE: &str = include_str!("usage/probe.py");
/// The status line wrapper the probe installs on each machine.
pub const STATUSLINE: &str = include_str!("usage/statusline.py");
/// A report older than this is read again when the composer looks at it.
pub const TTL: Duration = Duration::from_secs(5 * 60);
/// A provider's numbers older than this say nothing about today: Claude's
/// cache keeps its last reading after the last thread closes.
pub const STALE: Duration = Duration::from_secs(24 * 60 * 60);
/// Candidates within this many points of headroom are a tie, settled by the
/// harness in use, then the preferred order, so the choice does not flap.
const TIE: u32 = 5;

/// The provider behind each harness, unless the config says otherwise.
pub const PROVIDERS: &[(&str, &str)] = &[("claude", "anthropic"), ("codex", "openai")];

pub fn provider_for(harness: &str, configured: &BTreeMap<String, String>) -> Option<String> {
    configured
        .get(harness)
        .cloned()
        .or_else(|| PROVIDERS.iter().find(|(h, _)| *h == harness).map(|(_, p)| p.to_string()))
}

/// One usage window: how much of it is spent and when it starts over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// `5h`, `week`, `3h`…
    pub label: String,
    pub used_percent: u32,
    #[serde(default)]
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub plan: Option<String>,
    /// A limit is reached: a window is spent, or the provider says so.
    #[serde(default)]
    pub reached: bool,
    /// The provider's own verdict that ordinary use is refused. Unlike a
    /// spent window, it is not cleared by a window's reset: only the next
    /// reading can lift it.
    #[serde(default)]
    pub refused: bool,
    /// Codex "full reset" credits the user can spend from Codex itself.
    #[serde(default)]
    pub resets_available: u32,
    /// Seconds since the epoch when the provider gave these numbers.
    #[serde(default)]
    pub read_at: u64,
    #[serde(default)]
    pub source: String,
}

impl Usage {
    /// Older than [`STALE`]: shown with its age, never chosen on.
    pub fn is_stale(&self, now: SystemTime) -> bool {
        crate::discovery::seconds(now).saturating_sub(self.read_at) > STALE.as_secs()
    }

    /// The windows still running: one past its reset has started over.
    pub fn live_windows(&self, now: SystemTime) -> Vec<&Window> {
        let now = crate::discovery::seconds(now);
        self.windows.iter().filter(|w| w.resets_at.is_none_or(|at| at > now)).collect()
    }

    /// The window that binds: the most spent, the sooner reset on a tie.
    fn tightest(&self, now: SystemTime) -> Option<&Window> {
        self.live_windows(now).into_iter().max_by_key(|w| (w.used_percent, std::cmp::Reverse(w.resets_at)))
    }

    /// Points left before the tightest window is spent: none while the
    /// provider refuses use, else what the running windows leave.
    pub fn headroom(&self, now: SystemTime) -> u32 {
        if self.refused {
            return 0;
        }
        100u32.saturating_sub(self.tightest(now).map(|w| w.used_percent).unwrap_or(0))
    }

    /// `5h 38% · week 29%`, or why nothing can run, with the reading's age
    /// when it is stale.
    pub fn summary(&self, now: SystemTime) -> String {
        let age = if self.is_stale(now) {
            format!(" · {} old", crate::ui::age(std::time::UNIX_EPOCH + Duration::from_secs(self.read_at), now))
        } else {
            String::new()
        };
        if self.headroom(now) == 0 {
            // A reset is promised only by an exhausted window; a refusal by
            // the provider's verdict ends with its next reading.
            let binding = self.tightest(now).filter(|w| w.used_percent >= 100).and_then(|w| w.resets_at);
            return match binding.map(|at| resets_in(at, now)) {
                Some(when) => format!("limit reached · resets {when}{age}"),
                None => format!("limit reached{age}"),
            };
        }
        let mut parts: Vec<String> =
            self.live_windows(now).iter().map(|w| format!("{} {}%", w.label, w.used_percent)).collect();
        if parts.is_empty() {
            return format!("every window reset{age}");
        }
        if self.resets_available > 0 {
            parts.push(format!("{} resets", self.resets_available));
        }
        format!("{}{age}", parts.join(" · "))
    }

    /// `62% left`, the one number the auto choice is made on.
    pub fn left(&self, now: SystemTime) -> String {
        format!("{}% left", self.headroom(now))
    }
}

/// `in 2h`, `in 35m`, `now`.
pub fn resets_in(at: u64, now: SystemTime) -> String {
    let now = crate::discovery::seconds(now);
    let secs = at.saturating_sub(now);
    match secs {
        0 => "now".into(),
        1..=59 => "in under a minute".into(),
        60..=3599 => format!("in {}m", secs / 60),
        3600..=86_399 => format!("in {}h", secs / 3600),
        _ => format!("in {}d", secs / 86_400),
    }
}

/// What the probe reported for one provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One machine's usage, every provider the probe looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    #[serde(flatten)]
    pub providers: BTreeMap<String, Slot>,
    /// Where the status line wrapper landed on that machine.
    #[serde(default)]
    pub statusline: Option<String>,
    /// Seconds since the epoch when the probe ran.
    #[serde(default)]
    pub read_at: u64,
}

impl Report {
    pub fn usage(&self, provider: &str) -> Option<&Usage> {
        self.providers.get(provider)?.usage.as_ref()
    }

    pub fn error(&self, provider: &str) -> Option<&str> {
        self.providers.get(provider)?.error.as_deref()
    }

    pub fn fresh(&self, now: SystemTime) -> bool {
        crate::discovery::seconds(now).saturating_sub(self.read_at) < TTL.as_secs()
    }
}

/// The harness with the most headroom among `candidates` (harness, its
/// provider's usage). None when no candidate has current usage, or every
/// one is spent: the composer then picks as it would without usage.
pub fn pick<'a>(
    candidates: &[(&'a str, Option<&Usage>)],
    current: Option<&str>,
    preferred: &[&str],
    now: SystemTime,
) -> Option<&'a str> {
    let usable: Vec<(&str, u32)> = candidates
        .iter()
        .filter_map(|(harness, usage)| usage.filter(|u| !u.is_stale(now)).map(|u| (*harness, u.headroom(now))))
        .filter(|(_, headroom)| *headroom > 0)
        .collect();
    let best = usable.iter().map(|(_, h)| *h).max()?;
    let tied: Vec<&'a str> = usable.iter().filter(|(_, h)| best - *h <= TIE).map(|(harness, _)| *harness).collect();
    tied.iter()
        .find(|t| Some(**t) == current)
        .or_else(|| preferred.iter().find_map(|p| tied.iter().find(|t| *t == p)))
        .or_else(|| tied.first())
        .copied()
}

/// The probe's one JSON argument. `state_dir` is this machine's; a remote
/// machine derives its own from its environment.
pub fn settings(state_dir: Option<&str>, codex: Option<&str>, claude: bool) -> String {
    json!({"state_dir": state_dir, "codex": codex, "claude": claude, "statusline": STATUSLINE}).to_string()
}

/// Turns the probe's output into a report.
pub fn finish(output: &str, now: SystemTime) -> Result<Report, String> {
    let json =
        output.lines().rev().find(|line| line.trim_start().starts_with('{')).ok_or("the probe printed nothing")?;
    let mut report: Report = serde_json::from_str(json).map_err(|err| format!("unreadable probe output: {err}"))?;
    report.read_at = crate::discovery::seconds(now);
    Ok(report)
}

/// The `--settings` Claude Code takes for one session: the wrapper as its
/// status line, re-run every minute so the numbers follow the session.
pub fn statusline_settings(wrapper: &str) -> String {
    let cache = match wrapper.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/anthropic.json"),
        None => "anthropic.json".into(),
    };
    json!({"statusLine": {"type": "command", "command": format!("python3 \"{wrapper}\" \"{cache}\""), "refreshInterval": 60}})
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn usage(windows: &[(&str, u32)]) -> Usage {
        Usage {
            windows: windows
                .iter()
                .map(|(l, u)| Window { label: l.to_string(), used_percent: *u, resets_at: None })
                .collect(),
            ..Usage::default()
        }
    }

    #[test]
    fn headroom_is_what_the_tightest_window_leaves() {
        assert_eq!(usage(&[("5h", 38), ("week", 71)]).headroom(at(0)), 29);
        assert_eq!(usage(&[]).headroom(at(0)), 100);
        assert_eq!(Usage { refused: true, ..usage(&[("5h", 10)]) }.headroom(at(0)), 0, "the provider's verdict");
        assert_eq!(
            Usage { reached: true, ..usage(&[("5h", 10)]) }.headroom(at(0)),
            90,
            "reached without a spent window or a verdict is only history"
        );
        assert_eq!(usage(&[("week", 120)]).headroom(at(0)), 0);
        // A window past its reset has started over and no longer counts.
        let reset = Usage {
            windows: vec![
                Window { label: "5h".into(), used_percent: 100, resets_at: Some(500) },
                Window { label: "week".into(), used_percent: 40, resets_at: Some(9000) },
            ],
            reached: true,
            ..Usage::default()
        };
        assert_eq!(reset.headroom(at(400)), 0);
        assert_eq!(reset.headroom(at(600)), 60, "the 5h window reset: only the week binds");
        assert_eq!(reset.summary(at(600)), "week 40%");
        // The provider's own refusal outlives a window's reset: only the
        // next reading can lift it.
        let refused = Usage { refused: true, ..reset.clone() };
        assert_eq!(refused.headroom(at(600)), 0);
        assert_eq!(refused.summary(at(600)), "limit reached", "no window is spent: no reset to promise");
        assert_eq!(refused.summary(at(400)), "limit reached · resets in 1m", "the spent 5h window's reset");
        assert!(!usage(&[]).is_stale(at(STALE.as_secs())));
        assert!(usage(&[]).is_stale(at(STALE.as_secs() + 1)));
    }

    #[test]
    fn summaries_read_as_windows_or_a_reached_limit() {
        assert_eq!(usage(&[("5h", 38), ("week", 29)]).summary(at(0)), "5h 38% · week 29%");
        assert_eq!(Usage { resets_available: 2, ..usage(&[("week", 17)]) }.summary(at(0)), "week 17% · 2 resets");
        assert_eq!(usage(&[("5h", 38)]).left(at(0)), "62% left");
        let reached = Usage {
            reached: true,
            windows: vec![Window { label: "5h".into(), used_percent: 100, resets_at: Some(1000 + 7200) }],
            ..Usage::default()
        };
        assert_eq!(reached.summary(at(1000)), "limit reached · resets in 2h");
        assert_eq!(Usage { refused: true, ..Usage::default() }.summary(at(0)), "limit reached");
        // The exhausted window's reset is the one that matters, not the soonest.
        let weekly = Usage {
            reached: true,
            windows: vec![
                Window { label: "5h".into(), used_percent: 30, resets_at: Some(1000 + 3600) },
                Window { label: "week".into(), used_percent: 100, resets_at: Some(1000 + 2 * 86_400) },
            ],
            ..Usage::default()
        };
        assert_eq!(weekly.summary(at(1000)), "limit reached · resets in 2d");
        let old = Usage { read_at: 100, ..usage(&[("week", 29)]) };
        assert_eq!(old.summary(at(100 + 3 * 86_400)), "week 29% · 3d old");
        assert_eq!(resets_in(100, at(100)), "now");
        assert_eq!(resets_in(100 + 30, at(100)), "in under a minute");
        assert_eq!(resets_in(100 + 35 * 60, at(100)), "in 35m");
        assert_eq!(resets_in(100 + 3 * 86_400, at(100)), "in 3d");
    }

    #[test]
    fn the_pick_takes_the_most_headroom_and_sticks_on_ties() {
        let claude = usage(&[("5h", 38), ("week", 29)]);
        let codex = usage(&[("week", 17)]);
        let now = at(0);
        let candidates = [("claude", Some(&claude)), ("codex", Some(&codex)), ("pi", None)];
        assert_eq!(pick(&candidates, Some("claude"), &["claude"], now), Some("codex"), "83 left beats 62");
        let close = usage(&[("week", 40)]);
        let candidates = [("claude", Some(&claude)), ("codex", Some(&close))];
        assert_eq!(
            pick(&candidates, Some("claude"), &["codex"], now),
            Some("claude"),
            "within 5 points, the current one stays"
        );
        assert_eq!(pick(&candidates, None, &["codex"], now), Some("codex"), "else the preferred order");
        assert_eq!(pick(&candidates, Some("pi"), &[], now), Some("claude"), "else the first candidate");
        let spent = Usage { reached: true, ..usage(&[("5h", 100)]) };
        let candidates = [("claude", Some(&spent)), ("codex", Some(&codex))];
        assert_eq!(
            pick(&candidates, Some("claude"), &["claude"], now),
            Some("codex"),
            "a reached limit is never picked"
        );
        let candidates = [("claude", Some(&spent)), ("codex", None)];
        assert_eq!(pick(&candidates, None, &[], now), None, "nothing usable: the composer falls back");
        assert_eq!(pick(&[], None, &[], now), None);
        // A days-old reading says nothing about today: it is no candidate.
        let candidates = [("claude", Some(&claude)), ("codex", Some(&codex))];
        let later = at(STALE.as_secs() + 1);
        assert_eq!(pick(&candidates, Some("codex"), &[], later), None, "both stale");
        let fresh = Usage { read_at: STALE.as_secs(), ..claude.clone() };
        let candidates = [("claude", Some(&fresh)), ("codex", Some(&codex))];
        assert_eq!(pick(&candidates, Some("codex"), &[], later), Some("claude"), "only the current reading counts");
    }

    #[test]
    fn providers_come_from_the_table_or_the_config() {
        assert_eq!(provider_for("claude", &BTreeMap::new()).as_deref(), Some("anthropic"));
        assert_eq!(provider_for("codex", &BTreeMap::new()).as_deref(), Some("openai"));
        assert_eq!(provider_for("pi", &BTreeMap::new()), None);
        let configured = BTreeMap::from([("pi".to_string(), "anthropic".to_string())]);
        assert_eq!(provider_for("pi", &configured).as_deref(), Some("anthropic"));
    }

    #[test]
    fn the_probe_output_is_parsed_after_any_noise() {
        let output = "Last login: today\n{\"statusline\": \"/s/usage/statusline.py\", \"anthropic\": {\"error\": \"no Claude thread has reported yet\"}, \"openai\": {\"usage\": {\"windows\": [{\"label\": \"week\", \"used_percent\": 17, \"resets_at\": 1792305784}], \"plan\": \"prolite\", \"reached\": false, \"resets_available\": 3, \"read_at\": 5, \"source\": \"app-server\"}}}\n";
        let report = finish(output, at(100)).unwrap();
        assert_eq!(report.read_at, 100);
        assert_eq!(report.statusline.as_deref(), Some("/s/usage/statusline.py"));
        assert_eq!(report.error("anthropic"), Some("no Claude thread has reported yet"));
        assert!(report.usage("anthropic").is_none());
        let openai = report.usage("openai").unwrap();
        assert_eq!(openai.windows[0].label, "week");
        assert_eq!(openai.resets_available, 3);
        assert_eq!(openai.plan.as_deref(), Some("prolite"));
        assert!(report.fresh(at(100 + 299)));
        assert!(!report.fresh(at(100 + 300)));
        assert!(finish("nothing", at(1)).is_err());
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), report, "the cache reads back as is");
    }

    #[test]
    fn the_settings_name_the_wrapper_and_its_cache() {
        let flag = statusline_settings("/s/usage/statusline.py");
        let value: serde_json::Value = serde_json::from_str(&flag).unwrap();
        assert_eq!(value["statusLine"]["type"], "command");
        assert_eq!(value["statusLine"]["command"], "python3 \"/s/usage/statusline.py\" \"/s/usage/anthropic.json\"");
        assert_eq!(value["statusLine"]["refreshInterval"], 60);
        let probe: serde_json::Value = serde_json::from_str(&settings(Some("/s"), Some("codex"), true)).unwrap();
        assert_eq!(probe["state_dir"], "/s");
        assert_eq!(probe["codex"], "codex");
        assert_eq!(probe["claude"], true);
        assert_eq!(probe["statusline"], STATUSLINE);
    }

    /// Runs the real probe with a fake `codex` that speaks the app server
    /// protocol, and the real wrapper with a status line input.
    #[cfg(unix)]
    #[test]
    fn the_real_probe_reads_codex_and_the_wrapper_cache() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let codex = dir.path().join("codex");
        std::fs::write(
            &codex,
            "#!/bin/sh\n[ \"$1\" = app-server ] || exit 1\nread first\nread second\nread third\n\
             printf '%s\\n' '{\"id\":0,\"result\":{}}' '{\"method\":\"account/updated\"}' \
             '{\"id\":1,\"result\":{\"ordinaryUsageAllowed\":'\"${FAKE_ALLOWED:-true}\"',\"rateLimits\":{\"primary\":{\"usedPercent\":17,\"windowDurationMins\":10080,\"resetsAt\":1792305784},\"secondary\":{\"usedPercent\":4,\"windowDurationMins\":300,\"resetsAt\":null},\"planType\":\"prolite\",\"rateLimitReachedType\":'\"${FAKE_REACHED:-null}\"'},\"rateLimitResetCredits\":{\"availableCount\":3}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |settings: String| {
            let output = std::process::Command::new("python3")
                .arg("-c")
                .arg(PROBE)
                .arg(settings)
                .env("HOME", dir.path())
                .env("CODEX_HOME", dir.path().join("nohome"))
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            finish(&String::from_utf8_lossy(&output.stdout), at(1)).unwrap()
        };
        let report = run(settings(state.to_str(), codex.to_str(), true));
        let wrapper = report.statusline.clone().expect("the wrapper is installed");
        assert_eq!(wrapper, state.join("usage/statusline.py").to_string_lossy());
        assert_eq!(std::fs::read_to_string(&wrapper).unwrap(), STATUSLINE);
        let openai = report.usage("openai").expect("the fake app server answered");
        let labels: Vec<&str> = openai.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["week", "5h"]);
        assert_eq!(openai.windows[0].used_percent, 17);
        assert_eq!(openai.windows[0].resets_at, Some(1792305784));
        assert_eq!(openai.resets_available, 3);
        assert_eq!(openai.source, "app-server");
        assert!(!openai.reached);
        assert_eq!(report.error("anthropic"), Some("no Claude thread has reported yet"));
        // The app server's own verdict outranks the percentages.
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(PROBE)
            .arg(settings(state.to_str(), codex.to_str(), false))
            .env("HOME", dir.path())
            .env("FAKE_ALLOWED", "false")
            .output()
            .unwrap();
        let refused = finish(&String::from_utf8_lossy(&output.stdout), at(1)).unwrap();
        assert!(refused.usage("openai").unwrap().reached, "ordinaryUsageAllowed: false is a reached limit");
        assert!(refused.usage("openai").unwrap().refused, "and the provider's own verdict");
        // An explicit verdict of allowed outranks a stale reached type.
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(PROBE)
            .arg(settings(state.to_str(), codex.to_str(), false))
            .env("HOME", dir.path())
            .env("FAKE_ALLOWED", "true")
            .env("FAKE_REACHED", "\"primary_window\"")
            .output()
            .unwrap();
        let allowed = finish(&String::from_utf8_lossy(&output.stdout), at(1)).unwrap();
        assert!(!allowed.usage("openai").unwrap().refused, "allowed is the verdict");
        assert!(!allowed.usage("openai").unwrap().reached);

        // Claude Code runs the wrapper with its status line JSON; the user's
        // own status line still gets the same input.
        let claude_dir = dir.path().join("claude-config");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"statusLine": {"type": "command", "command": "printf 'mine: '; head -c 9"}}"#,
        )
        .unwrap();
        let cache = state.join("usage/anthropic.json");
        let mut wrapper_run = std::process::Command::new("python3");
        wrapper_run
            .arg(&wrapper)
            .arg(&cache)
            .env("CLAUDE_CONFIG_DIR", &claude_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = wrapper_run.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"model": 1, "rate_limits": {"five_hour": {"used_percentage": 23.5, "resets_at": 1738425600}, "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}}}"#)
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "mine: {\"model\":", "the user's status line is chained");
        assert_eq!(std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777, 0o600);
        // A project's own status line outranks the user's, as Claude Code
        // would have ordered them without `--settings`.
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude/settings.json"),
            r#"{"statusLine": {"type": "command", "command": "printf 'project'"}}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/settings.local.json"),
            r#"{"statusLine": {"type": "command", "command": "printf 'local'"}}"#,
        )
        .unwrap();
        let chained = |input: String| {
            let mut child = std::process::Command::new("python3")
                .arg(&wrapper)
                .arg(&cache)
                .env("CLAUDE_CONFIG_DIR", &claude_dir)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
        };
        let project_dir = project.to_string_lossy();
        assert_eq!(chained(format!(r#"{{"workspace": {{"project_dir": "{project_dir}"}}}}"#)), "local");
        std::fs::remove_file(project.join(".claude/settings.local.json")).unwrap();
        assert_eq!(chained(format!(r#"{{"cwd": "{project_dir}"}}"#)), "project");
        assert_eq!(
            chained(r#"{"cwd": "/nowhere"}"#.into()),
            "mine: {\"cwd\": \"",
            "the user's when the project has none"
        );
        // A user's own `statusline.py` is not this wrapper: it is chained.
        std::fs::write(claude_dir.join("statusline.py"), "import sys\nsys.stdout.write('user-py')\n").unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            format!(
                r#"{{"statusLine": {{"type": "command", "command": "python3 {}/statusline.py"}}}}"#,
                claude_dir.display()
            ),
        )
        .unwrap();
        assert_eq!(chained(r#"{"cwd": "/nowhere"}"#.into()), "user-py");
        // A user status line that runs this very wrapper ends the chain
        // instead of recursing.
        std::fs::write(
            claude_dir.join("settings.json"),
            format!(r#"{{"statusLine": {{"type": "command", "command": "python3 {wrapper} {}"}}}}"#, cache.display()),
        )
        .unwrap();
        assert_eq!(chained(r#"{"cwd": "/nowhere"}"#.into()), "");

        let report = run(settings(state.to_str(), None, true));
        assert!(report.usage("openai").is_none(), "codex was not asked for");
        let anthropic = report.usage("anthropic").expect("the cache is read");
        let windows: Vec<(&str, u32)> = anthropic.windows.iter().map(|w| (w.label.as_str(), w.used_percent)).collect();
        assert_eq!(windows, [("5h", 24), ("week", 41)]);
        assert_eq!(anthropic.windows[1].resets_at, Some(1738857600));
        assert_eq!(anthropic.source, "statusline");

        // A failing app server falls back to the newest session log.
        std::fs::write(&codex, "#!/bin/sh\nexit 1\n").unwrap();
        let sessions = dir.path().join("codex-home/sessions/2026/10/10");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("rollout-1.jsonl"),
            "{\"type\":\"session_meta\"}\n{\"timestamp\":\"2026-10-10T12:19:39.418Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":17.0,\"window_minutes\":10080,\"resets_at\":1791949778},\"secondary\":null,\"plan_type\":\"prolite\",\"rate_limit_reached_type\":null}}}\n",
        )
        .unwrap();
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(PROBE)
            .arg(settings(state.to_str(), codex.to_str(), false))
            .env("HOME", dir.path())
            .env("CODEX_HOME", dir.path().join("codex-home"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let report = finish(&String::from_utf8_lossy(&output.stdout), at(1)).unwrap();
        let openai = report.usage("openai").expect("the session log is read");
        assert_eq!(openai.source, "session");
        assert_eq!(openai.windows[0].used_percent, 17);
        assert_eq!(openai.read_at, 1791634779, "the event's own timestamp");
        assert!(!report.providers.contains_key("anthropic"), "claude was not asked for");
    }
}
