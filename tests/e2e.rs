//! End to end: a real Herdr server in a throwaway config directory, the real
//! `herdr-inbox` binary in a pseudo-terminal, and assertions on what the user
//! would see.
//!
//! Runs only when `HERDR_E2E=1`, because it needs a `herdr` binary
//! (`HERDR_BIN`, default `herdr` on PATH). Nothing touches the user's own
//! Herdr: every directory Herdr uses points into a temporary directory.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn enabled() -> bool {
    std::env::var("HERDR_E2E").is_ok_and(|v| v == "1")
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".into())
}

/// The directory holding the herdr binary, resolved once from the caller's PATH.
fn herdr_dir() -> String {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let bin = herdr_bin();
        let resolved = if bin.contains('/') {
            PathBuf::from(&bin)
        } else {
            std::env::var("PATH")
                .unwrap_or_default()
                .split(':')
                .map(|dir| Path::new(dir).join(&bin))
                .find(|candidate| candidate.is_file())
                .unwrap_or_else(|| PathBuf::from(&bin))
        };
        // Follow a mise or Homebrew shim to the real binary's directory.
        let real = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        real.parent().map(|p| p.display().to_string()).unwrap_or_default()
    })
    .clone()
}

/// A Herdr server whose every directory lives in a temp dir.
struct Sandbox {
    root: tempfile::TempDir,
    server: Child,
}

impl Sandbox {
    fn start() -> Self {
        // Unix socket paths are limited to ~100 bytes: keep the root short.
        let root = tempfile::Builder::new().prefix("hi-e2e").tempdir_in("/tmp").expect("tempdir");
        for dir in ["config", "state", "data", "cache", "home", "bin"] {
            std::fs::create_dir_all(root.path().join(dir)).expect("mkdir");
        }
        let mut command = Command::new(herdr_bin());
        command.arg("server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let sandbox_env = Self::env_for(root.path());
        apply_env(&mut command, &sandbox_env);
        let server = command.spawn().expect("start herdr server");
        let sandbox = Self { root, server };
        sandbox.wait_for(|| sandbox.socket().exists(), "the server socket");
        sandbox
    }

    /// Saves `remote` as an SSH machine in this sandbox's Herdr, the way
    /// `herdr machine add` would, without connecting anywhere.
    fn save_machine(&self, id: &str, label: &str) {
        let dir = self.root.path().join("state/herdr/client");
        std::fs::create_dir_all(&dir).expect("mkdir client state");
        let profiles = serde_json::json!({"version": 1, "ssh": [
            {"id": id, "label": label, "target": "studio", "session": "default", "enabled": true}
        ]});
        std::fs::write(dir.join("endpoints.json"), profiles.to_string()).expect("write endpoints");
        let listed = self.herdr(&["machine", "list", "--json"]);
        assert_eq!(listed[0]["label"], label, "herdr accepts the saved machine");
    }

    fn env_for(root: &Path) -> Vec<(String, String)> {
        vec![
            ("XDG_CONFIG_HOME".into(), root.join("config").display().to_string()),
            ("XDG_STATE_HOME".into(), root.join("state").display().to_string()),
            ("XDG_DATA_HOME".into(), root.join("data").display().to_string()),
            ("XDG_CACHE_HOME".into(), root.join("cache").display().to_string()),
            // Herdr puts worktrees under $HOME/.herdr: keep them in the sandbox.
            ("HOME".into(), root.join("home").display().to_string()),
            // Only the sandbox's fake agent CLIs, herdr, and the system tools
            // (sh, git, python3): the user's real agents must not be probed.
            ("PATH".into(), format!("{}:{}:/usr/local/bin:/usr/bin:/bin", root.join("bin").display(), herdr_dir())),
            ("SHELL".into(), "/bin/sh".into()),
            ("PS1".into(), "$ ".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]
    }

    fn env(&self) -> Vec<(String, String)> {
        Self::env_for(self.root.path())
    }

    fn socket(&self) -> PathBuf {
        self.root.path().join("config/herdr/herdr.sock")
    }

    fn herdr(&self, args: &[&str]) -> serde_json::Value {
        let mut command = Command::new(herdr_bin());
        command.args(args);
        apply_env(&mut command, &self.env());
        let output = command.output().expect("run herdr");
        assert!(output.status.success(), "herdr {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
    }

    fn wait_for(&self, condition: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let mut stop = Command::new(herdr_bin());
        stop.args(["server", "stop"]).stdout(Stdio::null()).stderr(Stdio::null());
        apply_env(&mut stop, &self.env());
        let _ = stop.status();
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

fn apply_env(command: &mut Command, env: &[(String, String)]) {
    for (key, _) in std::env::vars() {
        if key.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    for (key, value) in env {
        command.env(key, value);
    }
}

/// The inbox running in a pseudo-terminal, with its screen parsed as a
/// terminal would show it.
struct Inbox {
    screen: Arc<Mutex<vt100::Parser>>,
    changed: Arc<Condvar>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Inbox {
    fn start(sandbox: &Sandbox, cols: u16, rows: u16) -> Self {
        Self::start_with(sandbox, cols, rows, &[])
    }

    fn start_with(sandbox: &Sandbox, cols: u16, rows: u16, extra_env: &[(&str, &str)]) -> Self {
        let pty =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).expect("openpty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr-inbox"));
        command.args(["--herdr", &herdr_bin()]);
        for (key, _) in std::env::vars() {
            if key.starts_with("HERDR_") {
                command.env_remove(key);
            }
        }
        for (key, value) in sandbox.env() {
            command.env(key, value);
        }
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let child = pty.slave.spawn_command(command).expect("spawn herdr-inbox");
        drop(pty.slave);
        let screen = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let changed = Arc::new(Condvar::new());
        let writer = Arc::new(Mutex::new(pty.master.take_writer().expect("writer")));
        let mut reader = pty.master.try_clone_reader().expect("reader");
        {
            let screen = Arc::clone(&screen);
            let changed = Arc::clone(&changed);
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let chunk = &buf[..n];
                    let mut screen = screen.lock().unwrap();
                    screen.process(chunk);
                    changed.notify_all();
                    // Answer the queries a real terminal answers.
                    let mut answer = Vec::new();
                    if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                        let (row, col) = screen.screen().cursor_position();
                        answer.extend(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
                    }
                    if chunk.windows(3).any(|w| w == b"\x1b[c") {
                        answer.extend(b"\x1b[?62;22c");
                    }
                    if !answer.is_empty() {
                        let mut writer = writer.lock().unwrap();
                        let _ = writer.write_all(&answer);
                        let _ = writer.flush();
                    }
                }
            });
        }
        Self { screen, changed, writer, child, _master: pty.master }
    }

    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn wait_for_text(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let text = self.text();
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {needle:?}; screen:\n{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_until_gone(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.text().contains(needle) {
            assert!(Instant::now() < deadline, "{needle:?} never went away; screen:\n{}", self.text());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Writes keys with no pause after, for timing-sensitive input.
    fn press(&mut self, bytes: &str) {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes.as_bytes()).expect("write keys");
        writer.flush().expect("flush keys");
    }

    /// Measures ASCII input within one row, including a trailing space that
    /// cannot be distinguished from blank cells by a text-only assertion.
    fn press_and_measure(&mut self, bytes: &str) -> Duration {
        let expected = {
            let mut screen = self.screen.lock().unwrap();
            let started = Instant::now();
            while screen.screen().hide_cursor() {
                let remaining = Duration::from_secs(2).saturating_sub(started.elapsed());
                assert!(!remaining.is_zero(), "the input cursor never became visible");
                screen = self.changed.wait_timeout(screen, remaining).expect("visible cursor").0;
            }
            let (row, col) = screen.screen().cursor_position();
            (row, col + bytes.len() as u16)
        };
        // Catch the old 700 ms hold delay without a fragile sub-frame wall-clock
        // assertion on shared CI runners. The redraw tests use a controlled clock.
        let budget = Duration::from_millis(250);
        let started = Instant::now();
        self.press(bytes);
        let mut screen = self.screen.lock().unwrap();
        while screen.screen().hide_cursor() || screen.screen().cursor_position() != expected {
            let remaining = budget.saturating_sub(started.elapsed());
            assert!(
                !remaining.is_zero(),
                "key {bytes:?} exceeded {budget:?}; cursor {:?}, expected {expected:?}; screen:\n{}",
                screen.screen().cursor_position(),
                screen.screen().contents()
            );
            screen = self.changed.wait_timeout(screen, remaining).expect("screen changed").0;
        }
        let elapsed = started.elapsed();
        assert!(elapsed < budget, "key {bytes:?} took {elapsed:?}, budget {budget:?}");
        elapsed
    }

    fn keys(&mut self, bytes: &str) {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes.as_bytes()).expect("write keys");
        writer.flush().expect("flush keys");
        drop(writer);
        std::thread::sleep(Duration::from_millis(120));
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Writes an executable without this process holding it open for writing,
/// so a parallel test's fork cannot make it "text file busy".
fn write_executable(path: &Path, body: &str) {
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn sh");
    writer.stdin.take().expect("stdin").write_all(body.as_bytes()).expect("write script");
    assert!(writer.wait().expect("wait").success());
}

/// A real git repository with one commit, which `git worktree add` needs.
fn real_repo(root: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success());
    }
    repo
}

fn git_repo(root: &Path, name: &str, branch: &str) -> PathBuf {
    let repo = root.join(name);
    std::fs::create_dir_all(repo.join(".git")).expect("mkdir repo");
    std::fs::write(repo.join(".git/HEAD"), format!("ref: refs/heads/{branch}\n")).expect("HEAD");
    repo
}

#[test]
fn the_inbox_shows_threads_drives_an_agent_and_archives() {
    if !enabled() {
        eprintln!("skipped: set HERDR_E2E=1 to run against a real herdr");
        return;
    }
    let sandbox = Sandbox::start();
    let repo = git_repo(sandbox.root.path(), "cockpit", "fix-login");
    let created =
        sandbox.herdr(&["workspace", "create", "--cwd", repo.to_str().unwrap(), "--label", "cockpit", "--no-focus"]);
    let pane = created["result"]["root_pane"]["pane_id"].as_str().expect("pane id").to_string();
    sandbox.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "claude", "--state", "blocked", &pane]);
    sandbox.herdr(&["pane", "report-metadata", &pane, "--source", "e2e", "--token", "thread=Fix the login loop"]);

    let mut inbox = Inbox::start(&sandbox, 110, 24);
    inbox.wait_for_text("NEEDS INPUT");
    inbox.wait_for_text("Fix the login loop");
    inbox.wait_for_text("⎇ fix-login · Claude");

    // Enter focuses the agent; what we type runs in the pane.
    inbox.keys("\r");
    inbox.wait_for_text("AGENT");
    inbox.keys("echo inbox-$((40 + 2))\r");
    inbox.wait_for_text("inbox-42");

    // Status changes arrive live.
    sandbox.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "claude", "--state", "working", &pane]);
    inbox.wait_for_text("WORKING");
    inbox.wait_until_gone("NEEDS INPUT");

    // Back to the list, archive, confirm: the workspace closes in Herdr.
    inbox.keys("\t");
    inbox.wait_for_text("THREADS");
    inbox.keys("x");
    inbox.wait_for_text("Archive this thread?");
    inbox.keys("y");
    inbox.wait_for_text("Archived “Fix the login loop”");
    inbox.wait_for_text("No agent threads yet.");
    let workspaces = sandbox.herdr(&["workspace", "list"]);
    assert_eq!(workspaces["result"]["workspaces"].as_array().map(Vec::len), Some(0));
}

/// A fake `ssh` that runs the remote command locally, against `remote`'s
/// Herdr server: the inbox's whole SSH path, minus the network.
fn fake_ssh(local: &Sandbox, remote: &Sandbox) -> PathBuf {
    let home = remote.root.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");
    let herdr_dir = Command::new("sh")
        .args(["-c", &format!("dirname \"$(command -v {})\"", herdr_bin())])
        .output()
        .expect("locate herdr");
    let herdr_dir = String::from_utf8_lossy(&herdr_dir.stdout).trim().to_string();
    let mut exports = String::new();
    for (key, value) in remote.env() {
        exports.push_str(&format!("export {key}='{value}'\n"));
    }
    let script = format!(
        "#!/bin/sh\nwhile [ \"$1\" != \"-T\" ]; do shift; done\nshift\nshift\n{exports}export HOME='{home}'\nexport PATH='{herdr_dir}':\"$PATH\"\necho 'Last login: today'\nexec sh -c \"$1\"\n",
        home = home.display(),
    );
    let path = local.root.path().join("ssh");
    // Written by a child `sh`, so no parallel test thread can inherit an open
    // write handle and make the script "text file busy" when it runs.
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn sh");
    writer.stdin.take().expect("stdin").write_all(script.as_bytes()).expect("write fake ssh");
    assert!(writer.wait().expect("wait").success());
    path
}

#[test]
fn threads_on_a_saved_machine_open_live_over_ssh() {
    if !enabled() {
        return;
    }
    let local = Sandbox::start();
    let remote = Sandbox::start();
    let repo = git_repo(remote.root.path(), "api", "release");
    let created =
        remote.herdr(&["workspace", "create", "--cwd", repo.to_str().unwrap(), "--label", "api", "--no-focus"]);
    let pane = created["result"]["root_pane"]["pane_id"].as_str().expect("pane id").to_string();
    remote.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "codex", "--state", "blocked", &pane]);
    remote.herdr(&["pane", "report-metadata", &pane, "--source", "e2e", "--token", "thread=Ship the release"]);
    local.save_machine("0123456789abcdef0123456789abcdef", "Studio");
    let ssh = fake_ssh(&local, &remote);

    let mut inbox = Inbox::start_with(&local, 110, 24, &[("HERDR_INBOX_SSH", ssh.to_str().unwrap())]);
    inbox.wait_for_text("Ship the release");
    inbox.wait_for_text("Studio · Codex");
    inbox.wait_for_text("● Local  ● Studio");

    // The remote thread is the only one, so it opened on its own.
    inbox.keys("\r");
    inbox.wait_for_text("AGENT");
    inbox.keys("echo remote-$((40 + 2))\r");
    inbox.wait_for_text("remote-42");

    remote.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "codex", "--state", "working", &pane]);
    inbox.wait_for_text("WORKING");

    inbox.keys("\t");
    inbox.keys("x");
    inbox.wait_for_text("Archive this thread?");
    inbox.keys("y");
    inbox.wait_for_text("Archived “Ship the release”");
    let workspaces = remote.herdr(&["workspace", "list"]);
    assert_eq!(workspaces["result"]["workspaces"].as_array().map(Vec::len), Some(0));
}

#[test]
fn the_composer_discovers_projects_and_launches_into_a_new_worktree() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    let repo = real_repo(&root.join("work"), "cockpit");
    // An agent CLI that never becomes ready: the launch must fail cleanly,
    // after creating its worktree, and say why.
    write_executable(&root.join("bin/claude"), "#!/bin/sh\necho 'not really claude'\nexit 3\n");
    let config = format!("roots = [\"{}\"]\ndepth = 1\nagent_start_timeout_ms = 3500\n", root.join("work").display());
    std::fs::create_dir_all(root.join("config/herdr-inbox")).expect("mkdir config");
    std::fs::write(root.join("config/herdr-inbox/config.toml"), config).expect("write config");

    let mut inbox = Inbox::start(&sandbox, 120, 34);
    inbox.wait_for_text("No agent threads yet.");
    inbox.keys("n");
    inbox.wait_for_text("What should we build?");
    inbox.wait_for_text("Project    cockpit");
    inbox.wait_for_text("Harness    Claude");
    inbox.keys("Fix the login loop");
    inbox.wait_for_text("⎇ fix-login-loop");
    inbox.keys("\r");
    inbox.wait_for_text("LAUNCHES");
    inbox.wait_for_text("✗ cockpit · Claude");
    inbox.wait_for_text("timed out waiting for agent startup");

    // The worktree was created by Herdr, in the sandbox's home.
    let worktree = root.join("home/.herdr/worktrees/cockpit/fix-login-loop");
    assert!(worktree.is_dir(), "{} is missing", worktree.display());
    let branches =
        Command::new("git").args(["branch", "--list", "fix-login-loop"]).current_dir(&repo).output().expect("git");
    assert!(String::from_utf8_lossy(&branches.stdout).contains("fix-login-loop"));
    // The failed launch keeps its journal, with where it stopped.
    let journals: Vec<_> = std::fs::read_dir(root.join("state/herdr-inbox/launches")).expect("journals").collect();
    assert_eq!(journals.len(), 1);
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journals[0].as_ref().expect("entry").path()).expect("read"))
            .expect("json");
    assert_eq!(journal["stage"], "needs_attention");
    assert_eq!(journal["failed_stage"], "starting");
    assert_eq!(journal["branch"], "fix-login-loop");
}

#[test]
fn dictation_records_meters_and_types_the_transcript_into_the_composer() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    // A recorder that writes a real WAV header and samples, finalizing on SIGINT.
    write_executable(
        &root.join("bin/pw-record"),
        "#!/bin/sh\nfor out; do :; done\ntrap 'exit 0' INT\nprintf 'RIFF\\044\\000\\000\\000WAVEfmt \\020\\000\\000\\000\\001\\000\\001\\000\\200>\\000\\000\\000}\\000\\000\\002\\000\\020\\000data\\000\\000\\000\\000' > \"$out\"\nhead -c 64000 /dev/urandom >> \"$out\"\nwhile true; do sleep 0.05; done\n",
    );
    let config = "[speech]\ncommand = \"printf 'fix the login loop' # {file}\"\n";
    std::fs::create_dir_all(root.join("config/herdr-inbox")).expect("mkdir config");
    std::fs::write(root.join("config/herdr-inbox/config.toml"), config).expect("write config");

    let mut inbox = Inbox::start(&sandbox, 120, 30);
    inbox.wait_for_text("No agent threads yet.");
    inbox.keys("n");
    inbox.wait_for_text("What should we build?");
    inbox.keys("\x14");
    inbox.wait_for_text("● REC");
    inbox.wait_for_text("⌃T type");
    inbox.keys("\x14");
    inbox.wait_for_text("fix the login loop");
    inbox.wait_until_gone("● REC");
    let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .expect("tmp")
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("herdr-inbox-{}-", inbox.child.process_id().unwrap_or(0)))
        })
        .collect();
    assert!(leftovers.is_empty(), "the recording is deleted after transcription");
}

#[test]
fn without_a_server_the_inbox_offers_to_start_one() {
    if !enabled() {
        return;
    }
    let root = tempfile::Builder::new().prefix("hi-e2e").tempdir_in("/tmp").expect("tempdir");
    for dir in ["config", "state", "data", "cache", "home", "bin"] {
        std::fs::create_dir_all(root.path().join(dir)).expect("mkdir");
    }
    // No server yet; the Drop of this sandbox stops the one the inbox starts.
    let sandbox = Sandbox { root, server: Command::new("true").spawn().expect("true") };
    let mut inbox = Inbox::start(&sandbox, 80, 12);
    inbox.wait_for_text("No Herdr server is running.");
    inbox.keys("\r");
    inbox.wait_for_text("No agent threads yet.");
    assert!(sandbox.socket().exists());
}

#[test]
fn holding_space_dictates_into_the_composer_and_typed_spaces_still_type() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    write_executable(
        &root.join("bin/pw-record"),
        "#!/bin/sh\nfor out; do :; done\ntrap 'exit 0' INT\nprintf 'RIFF\\044\\000\\000\\000WAVEfmt \\020\\000\\000\\000\\001\\000\\001\\000\\200>\\000\\000\\000}\\000\\000\\002\\000\\020\\000data\\000\\000\\000\\000' > \"$out\"\nhead -c 64000 /dev/urandom >> \"$out\"\nwhile true; do sleep 0.05; done\n",
    );
    let config = "[speech]\nspace_hold = true\ncommand = \"printf 'the login loop' # {file}\"\n";
    std::fs::create_dir_all(root.join("config/herdr-inbox")).expect("mkdir config");
    std::fs::write(root.join("config/herdr-inbox/config.toml"), config).expect("write config");

    let mut inbox = Inbox::start(&sandbox, 120, 30);
    inbox.wait_for_text("No agent threads yet.");
    inbox.keys("n");
    inbox.wait_for_text("What should we build?");
    // Typed at a brisk pace, spaces included.
    for key in "fix it".chars() {
        inbox.press(&key.to_string());
        std::thread::sleep(Duration::from_millis(90));
    }
    inbox.wait_for_text("│ fix it ");
    // Hold the bar the way a keyboard repeats it: a pause, then 40 a second.
    inbox.press(" ");
    std::thread::sleep(Duration::from_millis(250));
    let held = Instant::now();
    let (mut recording, mut hint) = (false, false);
    let mut sent = Vec::new();
    while held.elapsed() < Duration::from_millis(1_500) {
        inbox.press(" ");
        sent.push(held.elapsed().as_millis());
        let text = inbox.text();
        recording |= text.contains("● REC");
        hint |= text.contains("release type");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(recording && hint, "recording while held; repeats sent at {sent:?} ms; screen:\n{}", inbox.text());
    // Let go: the words are typed after a single space, unsent.
    inbox.wait_for_text("│ fix it the login loop ");
    inbox.wait_until_gone("● REC");
    inbox.wait_for_text("What should we build?");
}

#[test]
fn paused_spaces_in_the_composer_do_not_wait_for_dictation() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let mut inbox = Inbox::start(&sandbox, 120, 34);
    inbox.wait_for_text("No agent threads yet.");
    inbox.keys("n");
    inbox.wait_for_text("What should we build?");
    let samples: Vec<_> = "a b c d e f".chars().map(|key| inbox.press_and_measure(&key.to_string())).collect();
    eprintln!("composer key-to-PTY-output latency: {samples:?}");
    inbox.wait_for_text("│ a b c d e f ");
}

#[test]
fn typing_stays_responsive_while_an_agent_repaints() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    let created = sandbox.herdr(&["workspace", "create", "--cwd", root.to_str().unwrap(), "--no-focus"]);
    let pane = created["result"]["root_pane"]["pane_id"].as_str().expect("pane id");
    sandbox.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "claude", "--state", "idle", pane]);
    sandbox.herdr(&["pane", "report-metadata", pane, "--source", "e2e", "--token", "thread=Typing probe"]);
    let fixture = root.join("typing.py");
    // Full-pane output at 60 fps, with an input line and a stable visible
    // cursor below it. Only the sandbox's shell ever runs this fixture.
    std::fs::write(
        &fixture,
        r#"import os, select, sys, termios, time, tty
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
text = ""
frame = 0
next_frame = time.monotonic()
try:
    sys.stdout.write("\x1b[2J")
    while True:
        ready, _, _ = select.select([fd], [], [], max(0, next_frame - time.monotonic()))
        if ready:
            data = os.read(fd, 4096)
            if not data or b"\x03" in data:
                break
            text += data.decode("utf-8", "replace")
        now = time.monotonic()
        if now >= next_frame or ready:
            cols, rows = os.get_terminal_size(fd)
            out = ["\x1b[?25l"]
            if now >= next_frame:
                for row in range(1, rows):
                    out.append(f"\x1b[{row};1H" + chr(65 + (frame + row) % 26) * (cols - 1))
                frame += 1
                next_frame = now + 1 / 60
            out.append(f"\x1b[{rows};1HBENCH> " + text + "\x1b[K\x1b[?25h")
            sys.stdout.write("".join(out))
            sys.stdout.flush()
finally:
    termios.tcsetattr(fd, termios.TCSANOW, old)
"#,
    )
    .expect("write typing fixture");

    let mut inbox = Inbox::start(&sandbox, 120, 34);
    inbox.wait_for_text("Typing probe");
    inbox.keys("\r");
    inbox.wait_for_text("AGENT");
    inbox.wait_for_text("$");
    inbox.keys(&format!("python3 -u '{}'\r", fixture.display()));
    inbox.wait_for_text("BENCH>");
    let samples: Vec<_> = "a b c d e f".chars().map(|key| inbox.press_and_measure(&key.to_string())).collect();
    eprintln!("streaming agent key-to-PTY-output latency: {samples:?}");
    inbox.wait_for_text("BENCH> a b c d e f");
    inbox.press("\x03");
    inbox.wait_for_text("$");
}
