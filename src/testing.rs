//! Test doubles. A fake Herdr server on a real Unix socket, so tests exercise
//! the same connection behaviour as production: one request line per
//! connection, and long-lived subscription connections.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

type Handler = dyn Fn(&Value, &mut UnixStream) + Send + Sync;

pub struct FakeServer {
    _dir: tempfile::TempDir,
    path: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
}

impl FakeServer {
    /// Answers every request with `{"id", "result": <handler output>}`, or
    /// closes the connection without answering when the handler returns None.
    pub fn start(handler: impl Fn(&Value) -> Option<Value> + Send + Sync + 'static) -> Self {
        Self::start_raw(move |request, stream| {
            if let Some(result) = handler(request) {
                write_line(stream, &json!({"id": request["id"], "result": result}));
            }
        })
    }

    /// Accepts connections and never answers.
    pub fn silent() -> Self {
        Self::start_raw(|_, _| thread::sleep(Duration::from_secs(5)))
    }

    /// Full control: the handler gets the parsed request and the stream, and may
    /// keep writing (subscriptions) for as long as it likes.
    pub fn start_raw(handler: impl Fn(&Value, &mut UnixStream) + Send + Sync + 'static) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind fake herdr socket");
        listener.set_nonblocking(true).expect("nonblocking listener");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        {
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // BSD sockets inherit the listener's non-blocking
                            // flag on accept; Linux ones do not.
                            if stream.set_nonblocking(false).is_err() {
                                continue;
                            }
                            let requests = Arc::clone(&requests);
                            let handler = Arc::clone(&handler);
                            thread::spawn(move || serve(stream, &requests, handler.as_ref()));
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        Self { _dir: dir, path, requests, stop }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(stream: UnixStream, requests: &Mutex<Vec<Value>>, handler: &Handler) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let Ok(request) = serde_json::from_str::<Value>(&line) else {
        return;
    };
    requests.lock().expect("requests lock").push(request.clone());
    handler(&request, &mut writer);
}

pub fn write_line(stream: &mut UnixStream, value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

/// Waits for a fixture's observable readiness instead of assuming a startup time.
#[track_caller]
pub fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "test fixture did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Writes an executable script without this process ever holding it open for
/// writing. Tests run in parallel threads; a thread that forks while another
/// holds a write handle passes that handle to its child, and executing the
/// script then fails with "text file busy" (ETXTBSY) until the child execs.
/// A short-lived `sh` does the writing instead, so no handle can leak.
pub fn write_executable(path: &Path, body: &str) {
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn sh to write a test script");
    child.stdin.take().expect("stdin").write_all(body.as_bytes()).expect("write test script");
    assert!(child.wait().expect("wait for sh").success(), "writing {} failed", path.display());
}

#[cfg(test)]
mod tests {
    use super::wait_until;

    #[test]
    fn ready_fixtures_are_checked_once() {
        let mut checks = 0;
        wait_until(|| {
            checks += 1;
            true
        });
        assert_eq!(checks, 1);
    }

    #[test]
    fn unready_fixtures_are_polled_until_they_are_ready() {
        let mut checks = 0;
        wait_until(|| {
            checks += 1;
            checks == 3
        });
        assert_eq!(checks, 3);
    }
}
