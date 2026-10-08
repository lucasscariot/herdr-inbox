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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn enabled() -> bool {
    std::env::var("HERDR_E2E").is_ok_and(|v| v == "1")
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".into())
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
        for dir in ["config", "state", "data", "cache"] {
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

    fn env_for(root: &Path) -> Vec<(String, String)> {
        vec![
            ("XDG_CONFIG_HOME".into(), root.join("config").display().to_string()),
            ("XDG_STATE_HOME".into(), root.join("state").display().to_string()),
            ("XDG_DATA_HOME".into(), root.join("data").display().to_string()),
            ("XDG_CACHE_HOME".into(), root.join("cache").display().to_string()),
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
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Inbox {
    fn start(sandbox: &Sandbox, cols: u16, rows: u16) -> Self {
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
        let child = pty.slave.spawn_command(command).expect("spawn herdr-inbox");
        drop(pty.slave);
        let screen = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let writer = Arc::new(Mutex::new(pty.master.take_writer().expect("writer")));
        let mut reader = pty.master.try_clone_reader().expect("reader");
        {
            let screen = Arc::clone(&screen);
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
        Self { screen, writer, child, _master: pty.master }
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

#[test]
fn without_a_server_the_inbox_offers_to_start_one() {
    if !enabled() {
        return;
    }
    let root = tempfile::Builder::new().prefix("hi-e2e").tempdir_in("/tmp").expect("tempdir");
    for dir in ["config", "state", "data", "cache"] {
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
