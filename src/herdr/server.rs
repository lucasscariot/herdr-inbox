//! Starts a Herdr server that outlives the inbox and the terminal it ran in.
//!
//! A process group of its own keeps the server out of the terminal's job
//! control, but on a systemd system that is not enough: Ghostty, GNOME
//! Terminal, Konsole and desktop launchers run each window or tab in its own
//! transient scope and stop that scope when it closes, which kills every
//! process in its cgroup whatever their session or group. So where a systemd
//! user manager runs, the server gets a scope of its own in `app.slice`,
//! named the way systemd's desktop integration names launched applications.
//! Elsewhere, or when `systemd-run` fails, the server starts directly.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::terminal::HerdrCommand;

/// How long `systemd-run` gets to move into its scope before it is given up
/// on. Longer than the 25 s a D-Bus call waits for the user manager's reply,
/// so a slow bus fails on its own first.
const SCOPE_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts `herdr server` in the background, exactly as `herdr` itself would,
/// and reaps it when it ends.
pub fn start(herdr: &HerdrCommand) {
    let systemd_run = scoped().then(|| OsString::from("systemd-run"));
    let herdr = herdr.clone();
    thread::spawn(move || run(&herdr, systemd_run, SCOPE_TIMEOUT));
}

/// Whether [`start`] tries to give the server a scope of its own here.
pub fn scoped() -> bool {
    user_manager(Path::new("/run/systemd/system"), |key| std::env::var_os(key))
}

fn run(herdr: &HerdrCommand, systemd_run: Option<OsString>, timeout: Duration) {
    if let Some(program) = systemd_run {
        let unit = unit_name();
        if let Ok(mut child) = scope_command(herdr, &program, &unit).spawn()
            && entered_scope(&mut child, &unit, timeout)
        {
            let _ = child.wait();
            return;
        }
    }
    if let Ok(mut child) = direct_command(herdr).spawn() {
        let _ = child.wait();
    }
}

/// Whether a systemd user manager can take a scope: the machine booted with
/// systemd (`sd_booted`) and the user bus is where `sd_bus_open_user` looks.
fn user_manager(booted: &Path, var: impl Fn(&str) -> Option<OsString>) -> bool {
    if !cfg!(target_os = "linux") || !booted.is_dir() {
        return false;
    }
    var("DBUS_SESSION_BUS_ADDRESS").is_some_and(|address| !address.is_empty())
        || var("XDG_RUNTIME_DIR").is_some_and(|dir| !dir.is_empty() && Path::new(&dir).join("bus").exists())
}

/// `app-<launcher>-<ApplicationID>-<RANDOM>.scope`, with the dash in the
/// launcher's name escaped as `systemd-escape` would.
fn unit_name() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.subsec_nanos()).unwrap_or(0);
    format!("app-herdr\\x2dinbox-herdr-{:x}{nanos:x}.scope", std::process::id())
}

/// `systemd-run --scope` registers itself in the new scope, then execs the
/// server there with the inbox's environment, so agents see the same PATH,
/// keys and SSH agent as when the server is started from a shell.
fn scope_command(herdr: &HerdrCommand, systemd_run: &OsString, unit: &str) -> Command {
    let mut command = Command::new(systemd_run);
    command.args(["--user", "--scope", "--quiet", "--collect", "--slice=app.slice"]);
    command.arg(format!("--unit={unit}"));
    command.arg(format!("--description={}", description(herdr)));
    command.arg("--").arg(&herdr.program).args(&herdr.prefix).arg("server");
    for (key, value) in &herdr.env {
        command.env(key, value);
    }
    detach(&mut command);
    command
}

fn direct_command(herdr: &HerdrCommand) -> Command {
    let mut command = herdr.command();
    command.arg("server");
    detach(&mut command);
    command
}

fn detach(command: &mut Command) {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so closing the inbox never stops the server.
        command.process_group(0);
    }
}

fn description(herdr: &HerdrCommand) -> String {
    let session = herdr.prefix.windows(2).find(|pair| pair[0] == "--session").map(|pair| pair[1].to_string_lossy());
    match session {
        Some(session) => format!("Herdr server (session {session})"),
        None => "Herdr server".into(),
    }
}

/// Whether `systemd-run` got the server its scope. It moves itself into
/// `unit` before exec'ing the server, so the scope exists once its cgroup is
/// `unit`; exiting unsuccessfully before that, however late, means the user
/// manager refused it. Past `timeout` it is killed and counts as refused.
fn entered_scope(child: &mut Child, unit: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        if in_unit(child.id(), unit) {
            return true;
        }
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(_) => return true,
        }
    }
}

/// Whether process `pid` runs in the cgroup of systemd unit `unit`.
fn in_unit(pid: u32, unit: &str) -> bool {
    let suffix = format!("/{unit}");
    std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .is_ok_and(|text| text.lines().any(|line| line.ends_with(&suffix)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::write_executable;

    fn herdr(program: &Path, session: Option<&str>) -> HerdrCommand {
        let mut command = HerdrCommand::new(program);
        if let Some(session) = session {
            command.prefix = vec!["--session".into(), session.into()];
        }
        command.env = vec![("HERDR_SOCKET_PATH".into(), "/h/herdr.sock".into())];
        command
    }

    fn args(command: &Command) -> Vec<String> {
        command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    fn wait_for(path: &Path) -> String {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if let Ok(text) = std::fs::read_to_string(path)
                && text.ends_with('\n')
            {
                return text;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("{} never appeared", path.display());
    }

    #[test]
    fn the_scope_runs_the_same_server_command_in_app_slice() {
        let herdr = herdr(Path::new("/bin/herdr"), Some("night"));
        let command = scope_command(&herdr, &"systemd-run".into(), "app-herdr\\x2dinbox-herdr-1.scope");
        assert_eq!(command.get_program(), "systemd-run");
        assert_eq!(
            args(&command),
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=app.slice",
                "--unit=app-herdr\\x2dinbox-herdr-1.scope",
                "--description=Herdr server (session night)",
                "--",
                "/bin/herdr",
                "--session",
                "night",
                "server",
            ]
        );
        let env: Vec<_> = command.get_envs().collect();
        assert_eq!(env, [(std::ffi::OsStr::new("HERDR_SOCKET_PATH"), Some(std::ffi::OsStr::new("/h/herdr.sock")))]);
    }

    #[test]
    fn the_direct_fallback_is_plain_herdr_server() {
        let command = direct_command(&herdr(Path::new("/bin/herdr"), None));
        assert_eq!(command.get_program(), "/bin/herdr");
        assert_eq!(args(&command), ["server"]);
        assert_eq!(description(&herdr(Path::new("/bin/herdr"), None)), "Herdr server");
    }

    #[test]
    fn unit_names_follow_the_desktop_application_scheme() {
        let name = unit_name();
        let random = name.strip_prefix("app-herdr\\x2dinbox-herdr-").and_then(|rest| rest.strip_suffix(".scope"));
        let random = random.unwrap_or_else(|| panic!("unexpected unit name {name}"));
        assert!(!random.is_empty() && random.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
    }

    #[test]
    fn a_scope_needs_systemd_and_a_user_bus() {
        let root = tempfile::tempdir().expect("tempdir");
        let booted = root.path().join("systemd");
        let runtime = root.path().join("runtime");
        std::fs::create_dir_all(&runtime).expect("mkdir");
        let runtime_var = runtime.clone().into_os_string();
        let only_runtime = |key: &str| (key == "XDG_RUNTIME_DIR").then(|| runtime_var.clone());
        let bus_address = |key: &str| (key == "DBUS_SESSION_BUS_ADDRESS").then(|| OsString::from("unix:path=/x"));

        assert!(!user_manager(&booted, bus_address), "not booted with systemd");
        std::fs::create_dir_all(&booted).expect("mkdir");
        let linux = cfg!(target_os = "linux");
        assert_eq!(user_manager(&booted, bus_address), linux);
        assert!(!user_manager(&booted, only_runtime), "no bus socket in the runtime directory");
        std::fs::write(runtime.join("bus"), "").expect("bus");
        assert_eq!(user_manager(&booted, only_runtime), linux);
        assert!(!user_manager(&booted, |_| None), "no session at all");
    }

    #[test]
    fn the_server_starts_inside_the_scope_when_systemd_run_works() {
        let root = tempfile::tempdir().expect("tempdir");
        let log = root.path().join("log");
        let fake_run = root.path().join("systemd-run");
        // Records its own arguments, then execs what follows `--`, as `--scope` does.
        write_executable(
            &fake_run,
            &format!(
                "#!/bin/sh\nfor a; do shift; [ \"$a\" = -- ] && break; done\nprintf 'scope %s\\n' \"$HERDR_SOCKET_PATH\" >> {log:?}\nexec \"$@\"\n"
            ),
        );
        let fake_herdr = root.path().join("herdr");
        write_executable(&fake_herdr, &format!("#!/bin/sh\nprintf 'server %s\\n' \"$*\" >> {log:?}\n"));

        run(&herdr(&fake_herdr, Some("night")), Some(fake_run.into_os_string()), SCOPE_TIMEOUT);
        assert_eq!(wait_for(&log), "scope /h/herdr.sock\nserver --session night server\n");
    }

    #[test]
    fn a_scope_is_recognised_by_its_cgroup() {
        let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        let unit = own.lines().next().and_then(|line| line.rsplit('/').next()).unwrap_or_default().to_string();
        if !unit.is_empty() {
            assert!(in_unit(std::process::id(), &unit), "{own}");
        }
        assert!(!in_unit(std::process::id(), "app-herdr\\x2dinbox-herdr-0.scope"));
    }

    #[test]
    fn a_refused_scope_falls_back_to_a_direct_server() {
        let root = tempfile::tempdir().expect("tempdir");
        let log = root.path().join("log");
        let fake_run = root.path().join("systemd-run");
        write_executable(&fake_run, "#!/bin/sh\necho 'Failed to connect to bus' >&2\nexit 1\n");
        let fake_herdr = root.path().join("herdr");
        write_executable(&fake_herdr, &format!("#!/bin/sh\nprintf 'server %s\\n' \"$*\" >> {log:?}\n"));

        run(&herdr(&fake_herdr, None), Some(fake_run.into_os_string()), SCOPE_TIMEOUT);
        assert_eq!(wait_for(&log), "server server\n");

        let missing = root.path().join("no-systemd-run");
        std::fs::remove_file(&log).expect("rm");
        run(&herdr(&fake_herdr, None), Some(missing.into_os_string()), SCOPE_TIMEOUT);
        assert_eq!(wait_for(&log), "server server\n");
    }

    #[test]
    fn a_scope_refused_late_or_never_answered_still_falls_back() {
        let root = tempfile::tempdir().expect("tempdir");
        let log = root.path().join("log");
        let fake_herdr = root.path().join("herdr");
        write_executable(&fake_herdr, &format!("#!/bin/sh\nprintf 'server %s\\n' \"$*\" >> {log:?}\n"));

        // A slow user bus: the call times out long after systemd-run started.
        let slow = root.path().join("slow-systemd-run");
        write_executable(&slow, "#!/bin/sh\nsleep 2.5\necho 'Connection timed out' >&2\nexit 1\n");
        run(&herdr(&fake_herdr, None), Some(slow.into_os_string()), SCOPE_TIMEOUT);
        assert_eq!(wait_for(&log), "server server\n");

        // A bus that never answers: systemd-run is given up on.
        let stuck = root.path().join("stuck-systemd-run");
        write_executable(&stuck, "#!/bin/sh\nexec sleep 60\n");
        std::fs::remove_file(&log).expect("rm");
        let started = Instant::now();
        run(&herdr(&fake_herdr, None), Some(stuck.into_os_string()), Duration::from_millis(300));
        assert_eq!(wait_for(&log), "server server\n");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
