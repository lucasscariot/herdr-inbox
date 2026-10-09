use std::path::Path;

use serde_json::json;

use super::*;
use crate::herdr::events::Event;
use crate::testing::{FakeServer, write_line};

fn executable(path: &Path, body: &str) {
    crate::testing::write_executable(path, body);
}

/// A fake `ssh`: prints login noise, then runs the remote command with a
/// local `sh`, the way sshd runs it with the user's shell.
fn fake_ssh(dir: &Path) -> PathBuf {
    let ssh = dir.join("ssh");
    executable(
        &ssh,
        "#!/bin/sh\nwhile [ \"$1\" != \"-T\" ]; do shift; done\nshift\nshift\necho 'Last login: Thu Oct  8 on ttys001'\nexec sh -c \"$1\"\n",
    );
    ssh
}

/// A fake remote `herdr` in `$HOME/.local/bin`: `remote-api-bridge` pipes
/// stdio to the fake server's socket.
fn fake_remote_herdr(home: &Path, socket: &Path) {
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = format!(
        r#"#!/usr/bin/env python3
import socket, sys, threading
if sys.argv[1:] != ["remote-api-bridge"]:
    sys.stderr.write("unexpected args: %r\n" % sys.argv[1:]); sys.exit(2)
s = socket.socket(socket.AF_UNIX); s.connect({socket:?})
def up():
    for chunk in iter(lambda: sys.stdin.buffer.read1(4096), b""):
        s.sendall(chunk)
threading.Thread(target=up, daemon=True).start()
for chunk in iter(lambda: s.recv(4096), b""):
    sys.stdout.buffer.write(chunk); sys.stdout.buffer.flush()
"#,
        socket = socket.display().to_string()
    );
    executable(&bin.join("herdr"), &script);
}

struct Remote {
    _dir: tempfile::TempDir,
    transport: Transport,
}

fn remote(server: &FakeServer) -> Remote {
    let dir = tempfile::tempdir().unwrap();
    fake_remote_herdr(dir.path(), server.path());
    let ssh = SshHerdr {
        ssh: fake_ssh(dir.path()),
        target: "lucas@studio".into(),
        session: None,
        control_dir: dir.path().join("ctl"),
    };
    Remote { transport: Transport::Ssh(SshHerdr { ssh: wrap_home(dir.path(), &ssh.ssh), ..ssh }), _dir: dir }
}

/// Wraps the fake ssh so the "remote" side runs with `HOME` in the temp dir,
/// without touching this test process's environment.
fn wrap_home(home: &Path, ssh: &Path) -> PathBuf {
    let wrapper = home.join("ssh-home");
    executable(&wrapper, &format!("#!/bin/sh\nHOME={home:?} exec {ssh:?} \"$@\"\n", home = home, ssh = ssh));
    wrapper
}

#[test]
fn an_api_call_over_ssh_skips_login_noise_and_returns_the_result() {
    let server =
        FakeServer::start(|request| Some(json!({"type": "pong", "version": "0.9.3", "method": request["method"]})));
    let remote = remote(&server);
    let result = remote.transport.call("ping", json!({})).unwrap();
    assert_eq!(result["version"], "0.9.3");
    assert_eq!(server.requests()[0]["method"], "ping");
}

#[test]
fn server_errors_come_back_over_ssh_unchanged() {
    let server = FakeServer::start_raw(|request, stream| {
        write_line(stream, &json!({"id": request["id"], "error": {"code": "pane_not_found", "message": "no pane"}}));
    });
    let remote = remote(&server);
    let err = remote.transport.call("pane.get", json!({"pane_id": "w9:p9"})).unwrap_err();
    assert_eq!(err.code(), Some("pane_not_found"));
}

#[test]
fn an_unreachable_host_reports_what_ssh_said() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    executable(&ssh, "#!/bin/sh\necho 'ssh: connect to host studio port 22: Connection refused' >&2\nexit 255\n");
    let transport =
        Transport::Ssh(SshHerdr { ssh, target: "studio".into(), session: None, control_dir: dir.path().into() });
    let err = transport.call("ping", json!({})).unwrap_err();
    assert!(matches!(err, ApiError::Unreachable(_)), "{err:?}");
    assert_eq!(err.to_string(), "ssh: connect to host studio port 22: Connection refused");
    assert!(matches!(transport.subscribe(vec![json!({"type": "pane.created"})]), Err(ApiError::Unreachable(_))));
}

#[test]
fn a_missing_ssh_program_is_unreachable_not_a_panic() {
    let transport = Transport::Ssh(SshHerdr {
        ssh: "/nonexistent/ssh".into(),
        target: "studio".into(),
        session: None,
        control_dir: "/tmp".into(),
    });
    assert!(matches!(transport.call("ping", json!({})), Err(ApiError::Unreachable(_))));
}

#[test]
fn a_silent_remote_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    executable(&ssh, "#!/bin/sh\necho herdr-inbox-ready\nsleep 30\n");
    let ssh = SshHerdr { ssh, target: "studio".into(), session: None, control_dir: dir.path().into() };
    let started = std::time::Instant::now();
    let err = ssh_call(&ssh, "ping", json!({}), Duration::from_millis(300)).unwrap_err();
    assert!(matches!(err, ApiError::Unreachable(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_subscription_over_ssh_streams_events_until_closed() {
    let server = FakeServer::start_raw(|request, stream| {
        write_line(stream, &json!({"id": request["id"], "result": {"type": "subscription_started"}}));
        write_line(stream, &json!({"event": "pane_closed", "data": {}}));
        std::thread::sleep(Duration::from_secs(30));
    });
    let remote = remote(&server);
    let mut subscription = remote.transport.subscribe(vec![json!({"type": "pane.closed"})]).unwrap();
    assert_eq!(subscription.next_event().unwrap(), Some(Event::Lifecycle("pane.closed".into())));
    let closer = subscription.closer();
    let reader = thread::spawn(move || subscription.next_event());
    thread::sleep(Duration::from_millis(100));
    closer.close();
    assert!(matches!(reader.join().unwrap(), Ok(None) | Err(_)));
    assert_eq!(server.requests()[0]["method"], "events.subscribe");
}

#[test]
fn the_probe_output_is_parsed_into_checkouts() {
    let output = "/w/api/src\t/w/api\t/w/api/.git\tmain\n\
                  /h/.herdr/worktrees/cockpit/fix\t/h/.herdr/worktrees/cockpit/fix\t/w/cockpit/.git\tfix-login\n\
                  /w/detached\t/w/detached\t/w/detached/.git\tHEAD\n\
                  garbage line\n";
    let checkouts = parse_probe(output);
    assert_eq!(checkouts.len(), 3);
    assert_eq!(checkouts["/w/api/src"].repo, "api");
    assert_eq!(checkouts["/w/api/src"].root, PathBuf::from("/w/api"));
    assert_eq!(checkouts["/h/.herdr/worktrees/cockpit/fix"].repo, "cockpit");
    assert_eq!(checkouts["/h/.herdr/worktrees/cockpit/fix"].branch.as_deref(), Some("fix-login"));
    assert_eq!(checkouts["/w/detached"].branch, None);
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn the_remote_probe_reads_branches_and_worktrees_with_real_git() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("cockpit");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "x"]);
    let linked = dir.path().join("wt/fix login");
    git(&repo, &["worktree", "add", "-q", "-b", "fix-login", linked.to_str().unwrap()]);
    let ssh = SshHerdr { ssh: fake_ssh(dir.path()), target: "t".into(), session: None, control_dir: dir.path().into() };
    let paths = vec![
        repo.join("src").display().to_string(),
        linked.display().to_string(),
        dir.path().join("not-a-repo").display().to_string(),
    ];
    let checkouts = Transport::Ssh(ssh).checkouts(&paths);
    assert_eq!(checkouts.len(), 2, "{checkouts:?}");
    assert_eq!(checkouts[&paths[0]].repo, "cockpit");
    assert_eq!(checkouts[&paths[0]].branch.as_deref(), Some("main"));
    assert_eq!(checkouts[&paths[1]].repo, "cockpit", "a linked worktree is named after its main repository");
    assert_eq!(checkouts[&paths[1]].branch.as_deref(), Some("fix-login"));
}

#[test]
fn local_checkouts_use_the_git_reader() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("api");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let transport = Transport::Local { socket: dir.path().join("s"), herdr: HerdrCommand::new("herdr") };
    let checkouts = transport.checkouts(&[repo.display().to_string(), "/nonexistent".into()]);
    assert_eq!(checkouts.len(), 1);
    assert_eq!(checkouts[&repo.display().to_string()].branch.as_deref(), Some("main"));
}

#[test]
fn runners_build_local_and_remote_commands() {
    let local = Runner::Local(HerdrCommand::new("/bin/herdr"));
    let command = local.command(&["terminal", "session", "control", "w1:p1"]);
    assert_eq!(command.get_program(), "/bin/herdr");
    assert!(!local.has_marker());
    let remote = Runner::Ssh(SshHerdr::new("studio", None));
    let command = remote.command(&["agent", "list"]);
    assert_eq!(command.get_program(), "ssh");
    assert!(remote.has_marker());
}

#[test]
fn the_probe_runs_locally_and_over_ssh_with_the_same_result() {
    let script = "import sys, json; print(json.dumps({'arg': sys.argv[1]}))";
    let local = Transport::Local { socket: "/nowhere".into(), herdr: HerdrCommand::new("herdr") };
    let out = local.probe(script, "it's \"quoted\"", Duration::from_secs(10)).unwrap();
    assert_eq!(out.trim(), r#"{"arg": "it's \"quoted\""}"#);
    let dir = tempfile::tempdir().unwrap();
    let remote = Transport::Ssh(SshHerdr {
        ssh: fake_ssh(dir.path()),
        target: "t".into(),
        session: None,
        control_dir: dir.path().into(),
    });
    let out = remote.probe(script, "it's \"quoted\"", Duration::from_secs(10)).unwrap();
    assert_eq!(out.trim(), r#"{"arg": "it's \"quoted\""}"#, "login noise before the marker is dropped");
}

#[test]
fn a_failing_probe_reports_stderr_and_a_hung_one_times_out() {
    let local = Transport::Local { socket: "/nowhere".into(), herdr: HerdrCommand::new("herdr") };
    let err =
        local.probe("import sys; sys.stderr.write('boom\\n'); sys.exit(3)", "{}", Duration::from_secs(10)).unwrap_err();
    assert_eq!(err.to_string(), "boom");
    let started = std::time::Instant::now();
    let err = local.probe("import time; time.sleep(30)", "{}", Duration::from_millis(300)).unwrap_err();
    assert!(err.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(5));
}
