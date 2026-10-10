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
        Self::start_with(&herdr_bin()).unwrap_or_else(|error| panic!("{error}"))
    }

    fn start_with(binary: &str) -> Result<Self, String> {
        // Unix socket paths are limited to ~100 bytes: keep the root short.
        let root = tempfile::Builder::new().prefix("hi-e2e").tempdir_in("/tmp").expect("tempdir");
        for dir in ["config", "state", "data", "cache", "home", "bin"] {
            std::fs::create_dir_all(root.path().join(dir)).expect("mkdir");
        }
        let log = std::fs::File::create(root.path().join("server.log")).expect("server log");
        let mut command = Command::new(binary);
        command.arg("server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
        let sandbox_env = Self::env_for(root.path());
        // macOS's login /bin/sh runs path_helper, which otherwise drops our
        // fixture directory and can launch an installed agent instead. Restore
        // the isolated PATH after /etc/profile, only in this scratch HOME.
        let path = sandbox_env.iter().find(|(key, _)| key == "PATH").expect("sandbox PATH").1.as_str();
        std::fs::write(root.path().join("home/.profile"), format!("export PATH='{path}'\n")).expect("shell profile");
        apply_env(&mut command, &sandbox_env);
        let server = command.spawn().expect("start herdr server");
        let mut sandbox = Self { root, server };
        let api = herdr_inbox::herdr::api::Api::new(sandbox.socket()).with_timeout(Duration::from_millis(200));
        let deadline = Instant::now() + Duration::from_secs(15);
        while let Err(err) = api.ping() {
            let exited = sandbox.server.try_wait().expect("server status");
            if exited.is_some() || Instant::now() >= deadline {
                let log = std::fs::read_to_string(sandbox.root.path().join("server.log")).unwrap_or_default();
                return Err(format!(
                    "Herdr server did not become ready: exit={exited:?}; last ping: {err}; stderr:\n{log}"
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(sandbox)
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
        assert!(
            output.status.success(),
            "herdr {args:?} failed: {}; server stderr:\n{}",
            String::from_utf8_lossy(&output.stderr),
            std::fs::read_to_string(self.root.path().join("server.log")).unwrap_or_default()
        );
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
        Self::start_binary(sandbox, cols, rows, extra_env, Path::new(env!("CARGO_BIN_EXE_herdr-inbox")))
    }

    fn start_binary(sandbox: &Sandbox, cols: u16, rows: u16, extra_env: &[(&str, &str)], binary: &Path) -> Self {
        let pty =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).expect("openpty");
        let mut command = CommandBuilder::new(binary);
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

    fn click_text(&mut self, needle: &str, offset: u16) {
        use unicode_width::UnicodeWidthStr;
        let text = self.text();
        let (row, column) = text
            .lines()
            .enumerate()
            .find_map(|(row, line)| line.find(needle).map(|byte| (row as u16, line[..byte].width() as u16 + offset)))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{text}"));
        self.press(&format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", column + 1, row + 1, column + 1, row + 1));
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
fn updates_browse_and_install_a_checked_release_without_losing_the_draft_or_session() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    let bin = root.join("bin");
    let inbox_binary = bin.join("herdr-inbox");
    // Never replace the build's binary or the user's installed client.
    assert!(Command::new("cp").arg(env!("CARGO_BIN_EXE_herdr-inbox")).arg(&inbox_binary).status().unwrap().success());
    let stage = root.join("release-stage");
    std::fs::create_dir(&stage).unwrap();
    write_executable(&stage.join("herdr-inbox"), "#!/bin/sh\necho 'herdr-inbox 2.0.0'\n");
    let asset = herdr_inbox::update::asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let archive = root.join(&asset);
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("herdr-inbox")
            .status()
            .unwrap()
            .success()
    );
    let digest = Command::new("sh")
        .args(["-c", "if command -v sha256sum >/dev/null; then sha256sum \"$1\"; else shasum -a 256 \"$1\"; fi", "sh"])
        .arg(&archive)
        .output()
        .unwrap();
    let hash = String::from_utf8_lossy(&digest.stdout).split_whitespace().next().unwrap().to_string();
    std::fs::write(root.join(format!("{asset}.sha256")), format!("{hash}  {asset}\n")).unwrap();
    let json = serde_json::json!({
        "tag_name": "v2.0.0", "body": "A safer, faster inbox.",
        "assets": [{"name": asset}, {"name": format!("{asset}.sha256")}]
    });
    std::fs::write(root.join("release.json"), json.to_string()).unwrap();
    write_executable(
        &bin.join("curl"),
        &format!(
            "#!/bin/sh\nurl=\nout=\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    -o) out=\"$2\"; shift 2;;\n    https://*) url=\"$1\"; shift;;\n    *) shift;;\n  esac\ndone\ncase \"$url\" in\n  https://api.github.com/repos/lucasscariot/herdr-inbox/releases/latest)\n    if [ -f '{}/offline' ]; then echo 'network unavailable' >&2; exit 7; fi\n    cat '{}/release.json';;\n  https://github.com/lucasscariot/herdr-inbox/releases/download/v2.0.0/{asset}*)\n    cp '{}/'\"${{url##*/}}\" \"$out\";;\n  *) echo \"unexpected URL: $url\" >&2; exit 1;;\nesac\n",
            root.display(),
            root.display(),
            root.display()
        ),
    );
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    write_executable(&bin.join(opener), &format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}/browser.url'\n", root.display()));
    let config = root.join("config/herdr-inbox/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "roots = []\n").unwrap();
    let created = sandbox.herdr(&[
        "workspace",
        "create",
        "--cwd",
        root.to_str().unwrap(),
        "--label",
        "kept-thread",
        "--no-focus",
    ]);
    let pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    sandbox.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "claude", "--state", "idle", pane]);

    let mut inbox = Inbox::start_binary(&sandbox, 110, 30, &[], &inbox_binary);
    inbox.wait_for_text("THREADS");
    inbox.press("n");
    inbox.wait_for_text("NEW THREAD");
    inbox.press("Keep this draft");
    inbox.wait_for_text("Keep this draft");
    std::fs::write(root.join("offline"), "offline").unwrap();
    inbox.press("\x07");
    inbox.wait_for_text("Could not check GitHub");
    inbox.wait_for_text("network unavailable");
    std::fs::remove_file(root.join("offline")).unwrap();
    inbox.press("r");
    inbox.wait_for_text("Latest stable 2.0.0");
    inbox.wait_for_text("A safer, faster inbox.");
    let version = Command::new(&inbox_binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("herdr-inbox {}", env!("CARGO_PKG_VERSION")),
        "checking installs nothing"
    );
    inbox.press("b");
    sandbox.wait_for(|| root.join("browser.url").exists(), "the release page to open");
    assert_eq!(
        std::fs::read_to_string(root.join("browser.url")).unwrap(),
        "https://github.com/lucasscariot/herdr-inbox/releases/tag/v2.0.0"
    );
    inbox.press("\r");
    inbox.wait_for_text("Installed 2.0.0");
    inbox.wait_for_text("Restart Inbox");
    let version = Command::new(&inbox_binary).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "herdr-inbox 2.0.0");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "roots = []\n");
    let workspaces = sandbox.herdr(&["workspace", "list"]);
    assert_eq!(workspaces["result"]["workspaces"].as_array().map(Vec::len), Some(1), "Herdr still owns the thread");
    inbox.press("\x1b");
    inbox.wait_until_gone("Update Herdr Inbox");
    inbox.wait_for_text("Keep this draft");
    inbox.press("\x07");
    inbox.wait_for_text("Installed 2.0.0");
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
    inbox.wait_for_text("Claude · ⎇ fix-login");

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
fn navigating_tasks_shows_their_live_discussions_without_enter() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let repo = git_repo(sandbox.root.path(), "discussions", "main");
    for (title, state, marker) in
        [("First task", "blocked", "first-discussion"), ("Second task", "working", "second-discussion")]
    {
        let created =
            sandbox.herdr(&["workspace", "create", "--cwd", repo.to_str().unwrap(), "--label", title, "--no-focus"]);
        let pane = created["result"]["root_pane"]["pane_id"].as_str().expect("pane id");
        sandbox.herdr(&["pane", "run", pane, &format!("printf '\\033[2J\\033[H{marker}-%s\\n' 42")]);
        sandbox.herdr(&["pane", "report-agent", "--source", "e2e", "--agent", "claude", "--state", state, pane]);
        sandbox.herdr(&["pane", "report-metadata", pane, "--source", "e2e", "--token", &format!("thread={title}")]);
    }
    let mut inbox = Inbox::start(&sandbox, 110, 24);
    inbox.wait_for_text("First task");
    inbox.wait_for_text("Second task");
    inbox.wait_for_text("first-discussion-42");
    inbox.wait_for_text("THREADS");

    for (key, marker, previous) in [
        ("j", "second-discussion-42", "first-discussion-42"),
        ("\x1b[A", "first-discussion-42", "second-discussion-42"),
        ("\x1b[B", "second-discussion-42", "first-discussion-42"),
    ] {
        inbox.keys(key);
        inbox.wait_for_text(marker);
        inbox.wait_until_gone(previous);
        assert!(inbox.text().contains("THREADS"), "browsing keeps keyboard focus in the list");
    }
    inbox.keys("\t");
    inbox.wait_for_text("AGENT");
    inbox.keys("echo selected-$((40 + 2))\r");
    inbox.wait_for_text("selected-42");
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
fn the_sandbox_waits_for_a_serving_socket_not_just_a_bound_socket() {
    if !enabled() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let delayed = fixture.path().join("delayed-herdr");
    // Reproduce the bind/listen gap without relying on host scheduling. The
    // first CLI command must wait for the real server, not this socket inode.
    write_executable(
        &delayed,
        &format!(
            "#!/bin/sh\npython3 - <<'PY'\nimport os, socket, time\np = os.environ['XDG_CONFIG_HOME'] + '/herdr/herdr.sock'\nos.makedirs(os.path.dirname(p), exist_ok=True)\ns = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\ns.bind(p)\ntime.sleep(0.5)\ns.close()\nos.unlink(p)\nPY\nexec '{}' \"$@\"\n",
            herdr_bin()
        ),
    );
    let sandbox = Sandbox::start_with(delayed.to_str().unwrap()).expect("delayed server becomes ready");
    let created = sandbox.herdr(&[
        "workspace",
        "create",
        "--cwd",
        sandbox.root.path().to_str().unwrap(),
        "--label",
        "ready",
        "--no-focus",
    ]);
    assert!(created["result"]["root_pane"]["pane_id"].is_string());
}

#[test]
fn a_server_that_exits_during_startup_reports_its_exit_and_stderr() {
    if !enabled() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let failed = fixture.path().join("failed-herdr");
    write_executable(&failed, "#!/bin/sh\necho 'fixture startup failed' >&2\nexit 19\n");
    let started = Instant::now();
    let error = Sandbox::start_with(failed.to_str().unwrap()).err().expect("startup fails");
    assert!(error.contains("exit=Some(") && error.contains("fixture startup failed"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5), "do not wait out the 15-second readiness deadline after exit");
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
    inbox.wait_for_text("Codex · Studio");
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
fn composer_mouse_input_and_codex_thinking_work_in_the_real_terminal() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    real_repo(&root.join("work"), "cockpit");
    write_executable(
        &root.join("bin/codex"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --help ]; then printf '%s\\n' '--model <MODEL>' '-c, --config <key=value>'; exit 0; fi\nprintf '%s\\n' \"$@\" > '{}/codex-start.args'\necho 'not really codex'\nexit 3\n",
            root.display()
        ),
    );
    let codex_home = root.join("home/.codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(
        codex_home.join("models_cache.json"),
        serde_json::json!({"models": [{
            "slug": "gpt-test", "display_name": "GPT test", "visibility": "list",
            "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}, {"effort": "ultra"}]
        }]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(codex_home.join("config.toml"), "model = \"gpt-test\"\n").unwrap();
    std::fs::create_dir_all(root.join("config/herdr-inbox")).unwrap();
    std::fs::write(
        root.join("config/herdr-inbox/config.toml"),
        format!("roots = [\"{}\"]\ndepth = 1\nagent_start_timeout_ms = 3500\n", root.join("work").display()),
    )
    .unwrap();
    let mut inbox = Inbox::start_with(&sandbox, 120, 34, &[("CODEX_HOME", codex_home.to_str().unwrap())]);
    inbox.wait_for_text("No agent threads yet.");
    inbox.click_text("+  New thread", 3);
    inbox.wait_for_text("Model      Default (gpt-test)");
    inbox.click_text("Model      Default", 12);
    inbox.wait_for_text("╭ Model ");
    inbox.click_text("GPT test", 2);
    inbox.wait_until_gone("╭ Model ");
    inbox.wait_for_text("Model      GPT test");
    inbox.click_text("Thinking   Default", 12);
    inbox.wait_for_text("╭ Thinking ");
    inbox.click_text("ultra", 2);
    inbox.wait_until_gone("╭ Thinking ");
    inbox.wait_for_text("Thinking   ultra");
    inbox.click_text("Describe the task.", 0);
    inbox.press("Fix login");
    inbox.wait_for_text("Fix login");
    inbox.click_text("Fix login", 4);
    inbox.press("the ");
    inbox.wait_for_text("Fix the login");
    inbox.click_text("↵ send", 2);
    inbox.wait_for_text("✗ cockpit · Codex");
    let args = std::fs::read_to_string(root.join("codex-start.args"))
        .unwrap_or_else(|err| panic!("the sandbox Codex was not launched: {err}; screen:\n{}", inbox.text()));
    assert!(args.contains("--model\ngpt-test\n"), "{args}");
    assert!(args.contains("--config\nmodel_reasoning_effort=\"ultra\"\n"), "{args}");
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
fn comparing_agents_launches_the_task_once_per_agent_in_separate_worktrees() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    let repo = real_repo(&root.join("work"), "cockpit");
    // Two agent CLIs that never become ready: both launches must fail
    // cleanly, each after creating its own worktree.
    for cli in ["claude", "codex"] {
        write_executable(&root.join("bin").join(cli), &format!("#!/bin/sh\necho 'not really {cli}'\nexit 3\n"));
    }
    let config = format!("roots = [\"{}\"]\ndepth = 1\nagent_start_timeout_ms = 3500\n", root.join("work").display());
    std::fs::create_dir_all(root.join("config/herdr-inbox")).expect("mkdir config");
    std::fs::write(root.join("config/herdr-inbox/config.toml"), config).expect("write config");

    let mut inbox = Inbox::start(&sandbox, 120, 34);
    inbox.wait_for_text("No agent threads yet.");
    inbox.keys("n");
    inbox.wait_for_text("Harness    Claude");
    inbox.wait_for_text("Compare    Off · one agent");
    inbox.keys("Fix the login loop");
    inbox.wait_for_text("⎇ fix-login-loop");
    // F12 opens the comparison list; typing filters it to Codex.
    inbox.keys("\x1b[24~");
    inbox.wait_for_text(" Compare ");
    inbox.keys("codex");
    inbox.wait_for_text("its default model");
    inbox.keys("\r");
    inbox.wait_for_text("Compare    2 agents · also Codex");
    inbox.wait_for_text("⎇ fix-login-loop-{claude,codex}");
    inbox.keys("\r");
    inbox.wait_for_text("LAUNCHES");
    inbox.wait_for_text("✗ cockpit · Claude");
    inbox.wait_for_text("✗ cockpit · Codex");
    inbox.wait_for_text("Compare    Off · one agent");

    for branch in ["fix-login-loop-claude", "fix-login-loop-codex"] {
        let worktree = root.join("home/.herdr/worktrees/cockpit").join(branch);
        assert!(worktree.is_dir(), "{} is missing", worktree.display());
        let branches = Command::new("git").args(["branch", "--list", branch]).current_dir(&repo).output().expect("git");
        assert!(String::from_utf8_lossy(&branches.stdout).contains(branch));
    }
    let mut journals: Vec<serde_json::Value> = std::fs::read_dir(root.join("state/herdr-inbox/launches"))
        .expect("journals")
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.expect("entry").path()).expect("read")).expect("json"))
        .collect();
    journals.sort_by_key(|journal| journal["comparison"]["index"].as_u64());
    assert_eq!(journals.len(), 2);
    assert_eq!(journals[0]["harness"], "claude");
    assert_eq!(journals[0]["comparison"], serde_json::json!({"index": 0, "total": 2}));
    assert_eq!(journals[1]["harness"], "codex");
    assert_eq!(journals[1]["comparison"], serde_json::json!({"index": 1, "total": 2}));
    assert_ne!(journals[0]["agent_name"], journals[1]["agent_name"]);
    assert_eq!(journals[0]["task"], journals[1]["task"]);
}

#[test]
fn dictation_records_meters_and_types_the_transcript_into_the_composer() {
    if !enabled() {
        return;
    }
    let sandbox = Sandbox::start();
    let root = sandbox.root.path();
    // A recorder that writes a real WAV header and samples, finalizing on SIGINT.
    let ready = root.join("recorder-ready");
    write_executable(
        &root.join("bin/pw-record"),
        &format!(
            "#!/bin/sh\nfor out; do :; done\ntrap 'exit 0' INT\nprintf 'RIFF\\044\\000\\000\\000WAVEfmt \\020\\000\\000\\000\\001\\000\\001\\000\\200>\\000\\000\\000}}\\000\\000\\002\\000\\020\\000data\\000\\000\\000\\000' > \"$out\"\nhead -c 64000 /dev/urandom >> \"$out\"\nprintf ready > '{ready}'\nwhile true; do sleep 0.05; done\n",
            ready = ready.display(),
        ),
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
    sandbox.wait_for(|| ready.exists(), "the fake recorder's samples");
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
