//! A live, writable view of one pane through
//! `herdr terminal session control`, Herdr's documented interface for
//! third-party bridges.
//!
//! The subprocess prints one JSON record per line on stdout:
//! `terminal.frame` with base64 ANSI bytes, then `terminal.closed`. It reads
//! one JSON command per line on stdin. Running the `herdr` binary means the
//! private client protocol stays Herdr's business.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};

/// How to run `herdr` so it reaches the right server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCommand {
    pub program: PathBuf,
    /// Arguments placed before the subcommand, such as `--session x`.
    pub prefix: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl HerdrCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self { program: program.into(), prefix: Vec::new(), env: Vec::new() }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }

    pub fn control_args(target: &str, cols: u16, rows: u16) -> Vec<String> {
        vec![
            "terminal".into(),
            "session".into(),
            "control".into(),
            target.into(),
            "--takeover".into(),
            "--cols".into(),
            cols.to_string(),
            "--rows".into(),
            rows.to_string(),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    /// A full frame replaces the screen; otherwise it is a diff on the last one.
    pub full: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Frame(Frame),
    /// The stream ended. `reason` is Herdr's, or the subprocess's last error line.
    Closed {
        reason: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Record {
    #[serde(rename = "terminal.frame")]
    Frame {
        #[serde(default)]
        seq: u64,
        width: u16,
        height: u16,
        #[serde(default)]
        full: bool,
        #[serde(default)]
        encoding: Option<String>,
        bytes: String,
    },
    #[serde(rename = "terminal.closed")]
    Closed {
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Parses one stdout line. Unknown record types are skipped (`Ok(None)`).
pub fn parse_line(line: &str) -> Result<Option<Message>, String> {
    let record: Record = serde_json::from_str(line.trim()).map_err(|err| format!("invalid record: {err}"))?;
    match record {
        Record::Frame { seq, width, height, full, encoding, bytes } => {
            if let Some(encoding) = encoding.filter(|e| e != "ansi") {
                return Err(format!("unsupported frame encoding {encoding:?}"));
            }
            let bytes = BASE64.decode(bytes).map_err(|err| format!("invalid frame bytes: {err}"))?;
            Ok(Some(Message::Frame(Frame { seq, width, height, full, bytes })))
        }
        Record::Closed { reason } => Ok(Some(Message::Closed { reason })),
        Record::Other => Ok(None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Down,
    Up,
    Drag,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Scroll { direction: ScrollDirection, lines: u16 },
    Mouse { action: MouseAction, button: MouseButton, column: u16, row: u16, modifiers: u8 },
    Release,
}

/// Encodes one stdin command. Returns `None` for commands Herdr would reject.
pub fn encode(command: &Control) -> Option<String> {
    let value: Value = match command {
        Control::Input(bytes) if bytes.is_empty() => return None,
        Control::Input(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => json!({"type": "terminal.input", "text": text}),
            Err(_) => json!({"type": "terminal.input", "bytes": BASE64.encode(bytes)}),
        },
        Control::Resize { cols, rows } if *cols == 0 || *rows == 0 => return None,
        Control::Resize { cols, rows } => json!({"type": "terminal.resize", "cols": cols, "rows": rows}),
        Control::Scroll { lines: 0, .. } => return None,
        Control::Scroll { direction, lines } => json!({
            "type": "terminal.scroll",
            "direction": match direction { ScrollDirection::Up => "up", ScrollDirection::Down => "down" },
            "lines": lines,
        }),
        Control::Mouse { action, button, column, row, modifiers } => json!({
            "type": "terminal.mouse",
            "action": match action {
                MouseAction::Down => "down",
                MouseAction::Up => "up",
                MouseAction::Drag => "drag",
                MouseAction::Move => "move",
            },
            "button": match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
            },
            "column": column,
            "row": row,
            "modifiers": modifiers,
        }),
        Control::Release => json!({"type": "terminal.release"}),
    };
    Some(value.to_string())
}

/// One running controller subprocess.
pub struct Session {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
}

impl Session {
    /// Starts the controller and delivers every message to `on_message` from a
    /// reader thread. The last message is always `Message::Closed`.
    pub fn spawn(
        herdr: &HerdrCommand,
        target: &str,
        cols: u16,
        rows: u16,
        on_message: impl Fn(Message) + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut command = herdr.command();
        command
            .args(HerdrCommand::control_args(target, cols.max(1), rows.max(1)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("no stderr"))?;
        let last_error = Arc::new(Mutex::new(None::<String>));
        {
            let last_error = Arc::clone(&last_error);
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let line = line.trim().trim_start_matches("herdr:").trim().to_string();
                    if !line.is_empty() {
                        *last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(line);
                    }
                }
            });
        }
        let child = Arc::new(Mutex::new(child));
        {
            let child = Arc::clone(&child);
            thread::spawn(move || {
                let closed = read_stream(stdout, &on_message);
                // Give stderr a moment to deliver the line that explains an exit.
                let mut reason = closed.and_then(|reason| reason);
                if reason.is_none() {
                    let _ = wait_briefly(&child);
                    thread::sleep(Duration::from_millis(20));
                    reason = last_error.lock().unwrap_or_else(|e| e.into_inner()).take();
                }
                on_message(Message::Closed { reason });
            });
        }
        Ok(Self { child, stdin })
    }

    pub fn send(&mut self, command: &Control) -> std::io::Result<()> {
        let Some(line) = encode(command) else {
            return Ok(());
        };
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "session released"));
        };
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// Hands the pane back to Herdr. The subprocess exits on its own; it is
    /// killed if it lingers.
    pub fn release(&mut self) {
        let _ = self.send(&Control::Release);
        self.stdin = None;
        let child = Arc::clone(&self.child);
        thread::spawn(move || {
            if !wait_for_exit(&child, Duration::from_millis(500)) {
                let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
                let _ = child.kill();
                let _ = child.wait();
            }
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            self.release();
        }
    }
}

/// Reads records until EOF. Returns `Some(reason)` when Herdr sent
/// `terminal.closed`, `None` when the stream just ended.
fn read_stream(stdout: impl Read, on_message: &impl Fn(Message)) -> Option<Option<String>> {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            return None;
        };
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(&line) {
            Ok(Some(Message::Closed { reason })) => return Some(reason),
            Ok(Some(message)) => on_message(message),
            Ok(None) => {}
            Err(_) => {}
        }
    }
    None
}

fn wait_briefly(child: &Mutex<Child>) -> bool {
    wait_for_exit(child, Duration::from_millis(200))
}

fn wait_for_exit(child: &Mutex<Child>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(Some(_)) = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn frame_line(seq: u64, full: bool, bytes: &[u8]) -> String {
        json!({"type": "terminal.frame", "seq": seq, "encoding": "ansi", "width": 80, "height": 24,
               "full": full, "bytes": BASE64.encode(bytes)})
        .to_string()
    }

    #[test]
    fn frames_decode_their_base64_ansi_bytes() {
        let Some(Message::Frame(frame)) = parse_line(&frame_line(3, true, b"\x1b[1;1Hhi")).unwrap() else {
            panic!("expected a frame");
        };
        assert_eq!(frame, Frame { seq: 3, width: 80, height: 24, full: true, bytes: b"\x1b[1;1Hhi".to_vec() });
    }

    #[test]
    fn closed_records_keep_their_reason() {
        assert_eq!(
            parse_line(r#"{"type":"terminal.closed","reason":"taken over"}"#).unwrap(),
            Some(Message::Closed { reason: Some("taken over".into()) })
        );
        assert_eq!(parse_line(r#"{"type":"terminal.closed"}"#).unwrap(), Some(Message::Closed { reason: None }));
    }

    #[test]
    fn unknown_records_are_skipped_and_bad_ones_are_errors() {
        assert_eq!(parse_line(r#"{"type":"terminal.bell"}"#).unwrap(), None);
        assert!(parse_line("{").is_err());
        assert!(parse_line(r#"{"type":"terminal.frame","width":1,"height":1,"bytes":"%%"}"#).is_err());
        let other_encoding = r#"{"type":"terminal.frame","encoding":"cells","width":1,"height":1,"bytes":""}"#;
        assert!(parse_line(other_encoding).is_err());
    }

    #[test]
    fn utf8_input_is_sent_as_text_and_raw_bytes_as_base64() {
        assert_eq!(
            encode(&Control::Input("héllo\r".as_bytes().to_vec())).unwrap(),
            r#"{"text":"héllo\r","type":"terminal.input"}"#
        );
        let raw = encode(&Control::Input(vec![0xff, 0x1b])).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(BASE64.decode(value["bytes"].as_str().unwrap()).unwrap(), vec![0xff, 0x1b]);
        assert!(value.get("text").is_none());
    }

    #[test]
    fn commands_herdr_would_reject_are_not_sent() {
        assert_eq!(encode(&Control::Input(Vec::new())), None);
        assert_eq!(encode(&Control::Resize { cols: 0, rows: 10 }), None);
        assert_eq!(encode(&Control::Resize { cols: 10, rows: 0 }), None);
        assert_eq!(encode(&Control::Scroll { direction: ScrollDirection::Up, lines: 0 }), None);
    }

    #[test]
    fn resize_scroll_mouse_and_release_match_herdr_field_names() {
        let parse = |c: &Control| serde_json::from_str::<Value>(&encode(c).unwrap()).unwrap();
        assert_eq!(
            parse(&Control::Resize { cols: 120, rows: 40 }),
            json!({"type": "terminal.resize", "cols": 120, "rows": 40})
        );
        assert_eq!(
            parse(&Control::Scroll { direction: ScrollDirection::Down, lines: 3 }),
            json!({"type": "terminal.scroll", "direction": "down", "lines": 3})
        );
        assert_eq!(
            parse(&Control::Mouse {
                action: MouseAction::Drag,
                button: MouseButton::Right,
                column: 4,
                row: 2,
                modifiers: 2
            }),
            json!({"type": "terminal.mouse", "action": "drag", "button": "right", "column": 4, "row": 2, "modifiers": 2})
        );
        assert_eq!(parse(&Control::Release), json!({"type": "terminal.release"}));
    }

    #[test]
    fn control_args_take_over_at_the_requested_size() {
        assert_eq!(
            HerdrCommand::control_args("w1:p2", 100, 30),
            ["terminal", "session", "control", "w1:p2", "--takeover", "--cols", "100", "--rows", "30"]
        );
    }

    /// A stand-in for `herdr`: a shell script that prints canned records and
    /// copies stdin to a file so the test can read what was sent.
    fn fake_herdr(dir: &std::path::Path, stdout: &str, stderr: &str, exit: i32) -> HerdrCommand {
        let script = dir.join("herdr");
        let out = dir.join("out.jsonl");
        std::fs::write(&out, stdout).unwrap();
        let body = format!(
            "#!/bin/sh\necho \"$@\" > '{args}'\ncat '{out}'\nprintf '%s' '{stderr}' >&2\ncat > '{stdin}'\nexit {exit}\n",
            args = dir.join("args").display(),
            out = out.display(),
            stdin = dir.join("stdin").display(),
        );
        std::fs::write(&script, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        HerdrCommand::new(script)
    }

    /// Spawns, retrying while another test thread's fork still holds the
    /// freshly written script open (ETXTBSY), a race only tests that write
    /// executables can hit.
    fn spawn(
        herdr: &HerdrCommand,
        target: &str,
        (cols, rows): (u16, u16),
        on_message: impl Fn(Message) + Send + Clone + 'static,
    ) -> Session {
        for _ in 0..50 {
            match Session::spawn(herdr, target, cols, rows, on_message.clone()) {
                Ok(session) => return session,
                Err(err) if err.raw_os_error() == Some(26) => thread::sleep(Duration::from_millis(10)),
                Err(err) => panic!("spawn failed: {err}"),
            }
        }
        panic!("the fake herdr script stayed busy");
    }

    fn collect(rx: &mpsc::Receiver<Message>) -> Vec<Message> {
        let mut messages = Vec::new();
        while let Ok(message) = rx.recv_timeout(Duration::from_secs(3)) {
            let done = matches!(message, Message::Closed { .. });
            messages.push(message);
            if done {
                break;
            }
        }
        messages
    }

    #[test]
    fn a_session_streams_frames_then_closes_and_sends_commands() {
        let dir = tempfile::tempdir().unwrap();
        let stdout = format!("{}\n{}\n", frame_line(1, true, b"A"), frame_line(2, false, b"B"));
        let herdr = fake_herdr(dir.path(), &stdout, "", 0);
        let (tx, rx) = mpsc::channel();
        let mut session = spawn(&herdr, "w1:p1", (90, 20), move |m| {
            let _ = tx.send(m);
        });
        session.send(&Control::Input(b"ls\r".to_vec())).unwrap();
        session.release();
        let messages = collect(&rx);
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert!(matches!(&messages[0], Message::Frame(f) if f.bytes == b"A" && f.full));
        assert!(matches!(&messages[1], Message::Frame(f) if f.bytes == b"B" && !f.full));
        assert_eq!(messages[2], Message::Closed { reason: None });
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert_eq!(args.trim(), "terminal session control w1:p1 --takeover --cols 90 --rows 20");
        thread::sleep(Duration::from_millis(100));
        let sent = std::fs::read_to_string(dir.path().join("stdin")).unwrap();
        let lines: Vec<Value> = sent.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines, vec![json!({"type": "terminal.input", "text": "ls\r"}), json!({"type": "terminal.release"})]);
    }

    #[test]
    fn a_failing_session_closes_with_its_error_line() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = fake_herdr(dir.path(), "", "herdr: pane w9:p9 not found", 1);
        let (tx, rx) = mpsc::channel();
        let mut session = spawn(&herdr, "w9:p9", (80, 24), move |m| {
            let _ = tx.send(m);
        });
        session.release();
        assert_eq!(collect(&rx), vec![Message::Closed { reason: Some("pane w9:p9 not found".into()) }]);
    }

    #[test]
    fn herdr_close_reason_wins_over_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = fake_herdr(dir.path(), "{\"type\":\"terminal.closed\",\"reason\":\"taken over\"}\n", "noise", 0);
        let (tx, rx) = mpsc::channel();
        let mut session = spawn(&herdr, "w1:p1", (80, 24), move |m| {
            let _ = tx.send(m);
        });
        session.release();
        assert_eq!(collect(&rx), vec![Message::Closed { reason: Some("taken over".into()) }]);
    }

    #[test]
    fn sending_after_release_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = fake_herdr(dir.path(), "", "", 0);
        let mut session = spawn(&herdr, "w1:p1", (80, 24), |_| {});
        session.release();
        assert!(session.send(&Control::Input(b"x".to_vec())).is_err());
    }
}
