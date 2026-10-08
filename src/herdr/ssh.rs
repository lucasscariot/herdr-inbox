//! Running `herdr` on a saved SSH machine.
//!
//! Every command shares one SSH connection per host (ControlMaster), so after
//! the first one a remote call costs a round trip, not a handshake. The remote
//! side runs through `sh -lc` with the usual install directories on PATH,
//! because a non-interactive SSH session does not load the user's shell setup,
//! and prints a marker line before `exec herdr` so login noise on stdout is
//! never mistaken for Herdr's output.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Printed by the remote shell right before it runs `herdr`.
pub const READY_MARKER: &str = "herdr-inbox-ready";

/// Where `herdr` is commonly installed; prepended to the remote PATH.
const REMOTE_PATH: &str =
    "$HOME/.local/bin:$HOME/.local/share/mise/shims:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHerdr {
    /// The `ssh` program; tests substitute a fake.
    pub ssh: PathBuf,
    /// `user@host`, or a `Host` alias from the user's SSH config.
    pub target: String,
    /// The remote Herdr session; `None` for the default one.
    pub session: Option<String>,
    /// Holds the ControlMaster sockets. Kept short: socket paths are limited
    /// to about 100 bytes.
    pub control_dir: PathBuf,
}

impl SshHerdr {
    pub fn new(target: impl Into<String>, session: Option<String>) -> Self {
        Self { ssh: PathBuf::from("ssh"), target: target.into(), session, control_dir: default_control_dir() }
    }

    /// `ssh <options> <target> <remote command>` running `herdr <args>`.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.ssh);
        command.args(self.ssh_options());
        command.arg(&self.target);
        command.arg(self.remote_command(args));
        command
    }

    /// `ssh <options> <target> <remote command>` running a shell script with
    /// positional arguments, for probes that are not `herdr` calls.
    pub fn script(&self, script: &str, args: &[&str]) -> Command {
        let mut command = Command::new(&self.ssh);
        command.args(self.ssh_options());
        command.arg(&self.target);
        let mut remote = format!("sh -c {} herdr-inbox", quote(script));
        for arg in args {
            remote.push(' ');
            remote.push_str(&quote(arg));
        }
        command.arg(remote);
        command
    }

    fn ssh_options(&self) -> Vec<OsString> {
        let mut options: Vec<OsString> = Vec::new();
        for option in [
            "BatchMode=yes",
            "ConnectTimeout=8",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=2",
            "ControlMaster=auto",
            "ControlPersist=600",
        ] {
            options.push("-o".into());
            options.push(option.into());
        }
        options.push("-o".into());
        let mut control_path = OsString::from("ControlPath=");
        control_path.push(self.control_dir.join("%C"));
        options.push(control_path);
        // No pseudo-terminal: the streams are JSON lines, not a terminal.
        options.push("-T".into());
        options
    }

    /// The single string the remote login shell runs. Every argument is
    /// single-quoted, which bash, zsh and fish all read the same way.
    pub fn remote_command(&self, args: &[&str]) -> String {
        let script = format!("PATH=\"{REMOTE_PATH}:$PATH\"; printf '%s\\n' {READY_MARKER}; exec herdr \"$@\"");
        let mut remote = format!("sh -lc {} herdr", quote(&script));
        if let Some(session) = &self.session {
            remote.push_str(" --session ");
            remote.push_str(&quote(session));
        }
        for arg in args {
            remote.push(' ');
            remote.push_str(&quote(arg));
        }
        remote
    }
}

/// Single-quotes `value` for a POSIX shell, closing and escaping any quote.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `/tmp/herdr-inbox-<uid>`, private to the user.
pub fn default_control_dir() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    PathBuf::from("/tmp").join(format!("herdr-inbox-{user}"))
}

/// Creates the control directory with owner-only permissions; SSH refuses a
/// ControlPath others could write to.
pub fn prepare_control_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        }
        Err(err) => Err(err),
    }
}

/// Drops lines before the ready marker. Returns `true` once the marker has
/// been seen, so callers can tell login noise from Herdr output.
pub fn is_marker(line: &str) -> bool {
    line.trim_end() == READY_MARKER
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn quoting_survives_quotes_spaces_and_dollars() {
        assert_eq!(quote("w1:p1"), "'w1:p1'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME `x` \"y\""), "'$HOME `x` \"y\"'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn the_command_shares_one_connection_and_never_prompts() {
        let ssh = SshHerdr {
            ssh: "ssh".into(),
            target: "lucas@mac-studio".into(),
            session: None,
            control_dir: "/tmp/herdr-inbox-lucas".into(),
        };
        let argv = args(&ssh.command(&["agent", "list"]));
        let joined = argv.join(" ");
        for option in
            ["BatchMode=yes", "ControlMaster=auto", "ControlPersist=600", "ControlPath=/tmp/herdr-inbox-lucas/%C"]
        {
            assert!(joined.contains(option), "{option} missing from {joined}");
        }
        assert_eq!(argv[argv.len() - 3], "-T");
        assert_eq!(argv[argv.len() - 2], "lucas@mac-studio");
        assert!(argv.last().unwrap().ends_with(" herdr 'agent' 'list'"), "{}", argv.last().unwrap());
    }

    #[test]
    fn a_named_session_is_passed_to_the_remote_herdr() {
        let ssh = SshHerdr::new("host", Some("night".into()));
        assert!(ssh.remote_command(&["ping"]).ends_with(" herdr --session 'night' 'ping'"));
    }

    #[test]
    fn the_remote_command_runs_through_a_login_sh_with_a_marker() {
        let remote = SshHerdr::new("host", None).remote_command(&["remote-api-bridge"]);
        assert!(remote.starts_with("sh -lc '"), "{remote}");
        assert!(remote.contains("printf '\\''%s\\n'\\'' herdr-inbox-ready"), "{remote}");
        assert!(remote.contains("exec herdr \"$@\""), "{remote}");
        assert!(remote.contains("/opt/homebrew/bin"), "{remote}");
    }

    #[test]
    fn the_remote_command_runs_exactly_the_given_args_in_a_real_shell() {
        // Run the remote string with a local sh, `herdr` replaced by printf,
        // to prove the quoting delivers each argument intact.
        let dir = tempfile::tempdir().unwrap();
        // `$HOME/.local/bin` comes first on the remote PATH: put the fake there.
        let bin = dir.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("herdr");
        crate::testing::write_executable(&fake, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]\\n' \"$a\"; done\n");
        let remote = SshHerdr::new("host", Some("s 1".into())).remote_command(&["it's", "a b", "$HOME"]);
        let output = Command::new("sh")
            .arg("-c")
            .arg(&remote)
            .env("HOME", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout, "herdr-inbox-ready\n[--session]\n[s 1]\n[it's]\n[a b]\n[$HOME]\n", "{stdout}");
    }

    #[test]
    fn scripts_get_their_arguments_as_positional_parameters() {
        let ssh = SshHerdr::new("host", None);
        let command = ssh.script("printf '%s|' \"$@\"", &["/a b", "c'd"]);
        let remote = args(&command).last().unwrap().clone();
        let output = Command::new("sh").arg("-c").arg(&remote).output().unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "/a b|c'd|");
    }

    #[test]
    fn the_control_dir_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("ctl");
        prepare_control_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&control).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_control_dir(&control).unwrap();
        assert_eq!(std::fs::metadata(&control).unwrap().permissions().mode() & 0o777, 0o700, "tightened again");
    }

    #[test]
    fn the_marker_is_recognised_with_or_without_carriage_return() {
        assert!(is_marker("herdr-inbox-ready"));
        assert!(is_marker("herdr-inbox-ready\r\n"));
        assert!(!is_marker("Welcome to macOS"));
    }
}
