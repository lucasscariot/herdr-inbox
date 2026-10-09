//! How one machine is reached: its JSON API, its event subscriptions, the way
//! to run `herdr` there, and git facts about its checkouts.
//!
//! Locally everything goes to the Unix socket directly. On a saved SSH machine
//! each API call runs `herdr remote-api-bridge`, which pipes stdio to the
//! remote socket, over a shared SSH connection.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::api::{Api, ApiError, next_request_id, parse_response};
use super::events::{Closer, Subscription, read_ack, subscribe_request};
use super::ssh::{SshHerdr, is_marker};
use super::terminal::HerdrCommand;
use crate::git::{self, Checkout};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs `herdr <args>` on a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runner {
    Local(HerdrCommand),
    Ssh(SshHerdr),
}

impl Runner {
    pub fn command(&self, args: &[&str]) -> Command {
        match self {
            Runner::Local(herdr) => {
                let mut command = herdr.command();
                command.args(args);
                command
            }
            Runner::Ssh(ssh) => ssh.command(args),
        }
    }

    /// Whether stdout starts with login noise ending in the ready marker.
    pub fn has_marker(&self) -> bool {
        matches!(self, Runner::Ssh(_))
    }

    pub fn program(&self) -> String {
        match self {
            Runner::Local(herdr) => herdr.program.display().to_string(),
            Runner::Ssh(ssh) => format!("{} {}", ssh.ssh.display(), ssh.target),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Transport {
    Local { socket: PathBuf, herdr: HerdrCommand },
    Ssh(SshHerdr),
}

impl Transport {
    pub fn runner(&self) -> Runner {
        match self {
            Transport::Local { herdr, .. } => Runner::Local(herdr.clone()),
            Transport::Ssh(ssh) => Runner::Ssh(ssh.clone()),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Transport::Local { .. })
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, ApiError> {
        match self {
            Transport::Local { socket, .. } => Api::new(socket.clone()).call(method, params),
            Transport::Ssh(ssh) => ssh_call(ssh, method, params, CALL_TIMEOUT),
        }
    }

    pub fn subscribe(&self, subscriptions: Vec<Value>) -> Result<Subscription, ApiError> {
        match self {
            Transport::Local { socket, .. } => Subscription::open(socket, subscriptions),
            Transport::Ssh(ssh) => ssh_subscribe(ssh, subscriptions),
        }
    }

    /// Repository and branch for each path that is inside a git checkout.
    pub fn checkouts(&self, paths: &[String]) -> HashMap<String, Checkout> {
        match self {
            Transport::Local { .. } => paths
                .iter()
                .filter_map(|path| git::checkout(std::path::Path::new(path)).map(|c| (path.clone(), c)))
                .collect(),
            Transport::Ssh(ssh) => ssh_checkouts(ssh, paths).unwrap_or_default(),
        }
    }
}

/// A running `herdr remote-api-bridge` over SSH.
struct Bridge {
    child: Arc<Mutex<Child>>,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    stderr: Arc<Mutex<Option<String>>>,
}

impl Bridge {
    fn spawn(ssh: &SshHerdr) -> Result<Self, ApiError> {
        let mut child = ssh
            .command(&["remote-api-bridge"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| ApiError::Unreachable(format!("cannot run {}: {err}", ssh.ssh.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| ApiError::Protocol("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| ApiError::Protocol("no stdout".into()))?;
        let stderr = last_line(child.stderr.take());
        Ok(Self { child: Arc::new(Mutex::new(child)), stdin, stdout: BufReader::new(stdout), stderr })
    }

    /// Skips login noise up to the marker. Ending first means SSH or the
    /// remote shell failed; its last stderr line says why.
    fn wait_ready(&mut self) -> Result<(), ApiError> {
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).wait();
                thread::sleep(Duration::from_millis(20));
                let reason = self.stderr.lock().unwrap_or_else(|e| e.into_inner()).take();
                return Err(ApiError::Unreachable(reason.unwrap_or_else(|| "the SSH connection closed".into())));
            }
            if is_marker(&line) {
                return Ok(());
            }
        }
    }

    fn kill(&self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Keeps the last non-empty stderr line of a process.
fn last_line(stderr: Option<std::process::ChildStderr>) -> Arc<Mutex<Option<String>>> {
    let last = Arc::new(Mutex::new(None));
    if let Some(stderr) = stderr {
        let last = Arc::clone(&last);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    *last.lock().unwrap_or_else(|e| e.into_inner()) = Some(line);
                }
            }
        });
    }
    last
}

fn ssh_call(ssh: &SshHerdr, method: &str, params: Value, timeout: Duration) -> Result<Value, ApiError> {
    let mut bridge = Bridge::spawn(ssh)?;
    let request = json!({"id": next_request_id(), "method": method, "params": params});
    let mut line = serde_json::to_vec(&request).map_err(|err| ApiError::Protocol(err.to_string()))?;
    line.push(b'\n');
    let (tx, rx) = mpsc::channel();
    let child = Arc::clone(&bridge.child);
    let stderr = Arc::clone(&bridge.stderr);
    thread::spawn(move || {
        let result = (|| {
            // Wait for the remote shell first: if SSH fails, writing would
            // only report a broken pipe instead of SSH's reason.
            bridge.wait_ready()?;
            bridge.stdin.write_all(&line)?;
            bridge.stdin.flush()?;
            let mut response = String::new();
            if bridge.stdout.read_line(&mut response)? == 0 {
                return Err(ApiError::Protocol("the remote Herdr closed without an answer".into()));
            }
            parse_response(&response)
        })();
        bridge.kill();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
            let _ = child.kill();
            let reason = stderr.lock().unwrap_or_else(|e| e.into_inner()).take();
            Err(ApiError::Unreachable(reason.unwrap_or_else(|| format!("{method} timed out"))))
        }
    }
}

fn ssh_subscribe(ssh: &SshHerdr, subscriptions: Vec<Value>) -> Result<Subscription, ApiError> {
    let mut bridge = Bridge::spawn(ssh)?;
    let request = subscribe_request(subscriptions)?;
    // A stuck handshake must not hang the link: kill the bridge after a while.
    let watchdog = Arc::clone(&bridge.child);
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        if done_rx.recv_timeout(CALL_TIMEOUT).is_err() {
            let _ = watchdog.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
    });
    let started = bridge
        .wait_ready()
        .and_then(|()| {
            bridge.stdin.write_all(&request)?;
            bridge.stdin.flush()?;
            Ok(())
        })
        .and_then(|()| read_ack(&mut bridge.stdout));
    let _ = done_tx.send(());
    if let Err(err) = started {
        bridge.kill();
        return Err(err);
    }
    let Bridge { child, stdin, stdout, .. } = bridge;
    // The bridge's stdin stays open for the subscription's lifetime: closing
    // it would end the remote connection.
    Ok(Subscription::from_parts(Box::new(KeepAlive { reader: stdout, _stdin: stdin }), Closer::process(child)))
}

/// A reader that keeps the bridge's stdin alive alongside it.
struct KeepAlive {
    reader: BufReader<std::process::ChildStdout>,
    _stdin: std::process::ChildStdin,
}

impl Read for KeepAlive {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl BufRead for KeepAlive {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

/// Reads repository and branch for many paths with one SSH round trip.
const PROBE: &str = r#"for p in "$@"; do
  root=$(git -C "$p" rev-parse --show-toplevel 2>/dev/null) || continue
  common=$(cd "$p" 2>/dev/null && cd "$(git rev-parse --git-common-dir 2>/dev/null)" 2>/dev/null && pwd -P) || continue
  branch=$(git -C "$p" symbolic-ref --short -q HEAD 2>/dev/null || git -C "$p" rev-parse --short HEAD 2>/dev/null)
  printf '%s\t%s\t%s\t%s\n' "$p" "$root" "$common" "$branch"
done
"#;

fn ssh_checkouts(ssh: &SshHerdr, paths: &[String]) -> Result<HashMap<String, Checkout>, ApiError> {
    if paths.is_empty() {
        return Ok(HashMap::new());
    }
    let args: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut child = ssh
        .script(PROBE, &args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| ApiError::Unreachable(err.to_string()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| ApiError::Protocol("no stdout".into()))?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        let _ = tx.send(output);
    });
    let output = rx.recv_timeout(PROBE_TIMEOUT);
    let _ = child.kill();
    let _ = child.wait();
    Ok(parse_probe(&output.map_err(|_| ApiError::Unreachable("git probe timed out".into()))?))
}

/// Parses `path<TAB>root<TAB>common-dir<TAB>branch` lines.
pub fn parse_probe(output: &str) -> HashMap<String, Checkout> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (path, root, common, branch) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let repo = git::repo_name(std::path::Path::new(common))?;
            let branch = Some(branch.trim()).filter(|b| !b.is_empty() && *b != "HEAD").map(str::to_string);
            Some((path.to_string(), Checkout { repo, branch, root: PathBuf::from(root) }))
        })
        .collect()
}

#[cfg(test)]
mod tests;
