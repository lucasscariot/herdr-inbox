//! Herdr's JSON socket API: newline-delimited JSON, one request per connection.
//!
//! The server reads only the first line of a connection, answers it and
//! closes, so every call opens a fresh connection.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::types::{Pong, SessionSnapshot};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Nothing is listening: the server is not running.
    #[error("no Herdr server is listening at {0}")]
    Unavailable(PathBuf),
    #[error("Herdr socket error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected answer from Herdr: {0}")]
    Protocol(String),
    #[error("{message}")]
    Server { code: String, message: String },
}

impl ApiError {
    pub fn code(&self) -> Option<&str> {
        match self {
            ApiError::Server { code, .. } => Some(code),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Api {
    socket: PathBuf,
    timeout: Duration,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_request_id() -> String {
    format!("inbox-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// Opens a connection, classifying "nothing listening" apart from other failures.
pub(crate) fn connect(socket: &Path) -> Result<UnixStream, ApiError> {
    UnixStream::connect(socket).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            ApiError::Unavailable(socket.to_path_buf())
        }
        _ => ApiError::Io(err),
    })
}

/// Turns one response line into its `result` object or the server's error.
pub(crate) fn parse_response(line: &str) -> Result<Value, ApiError> {
    let mut value: Value =
        serde_json::from_str(line.trim()).map_err(|err| ApiError::Protocol(format!("invalid JSON ({err})")))?;
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_str).unwrap_or("error");
        let message =
            error.get("message").and_then(Value::as_str).unwrap_or("Herdr returned an error without a message");
        return Err(ApiError::Server { code: code.to_string(), message: message.to_string() });
    }
    match value.get_mut("result") {
        Some(result) => Ok(result.take()),
        None => Err(ApiError::Protocol("response has neither result nor error".into())),
    }
}

impl Api {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into(), timeout: DEFAULT_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, ApiError> {
        let mut stream = connect(&self.socket)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let request = json!({"id": next_request_id(), "method": method, "params": params});
        let mut line = serde_json::to_vec(&request).map_err(|err| ApiError::Protocol(err.to_string()))?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.flush()?;
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        if reader.read_line(&mut response)? == 0 {
            return Err(ApiError::Protocol(format!("{method}: connection closed without an answer")));
        }
        parse_response(&response)
    }

    /// Calls a method and decodes one field of its result.
    fn call_field<T: DeserializeOwned>(&self, method: &str, params: Value, field: &str) -> Result<T, ApiError> {
        let mut result = self.call(method, params)?;
        let value = result
            .get_mut(field)
            .map(Value::take)
            .ok_or_else(|| ApiError::Protocol(format!("{method}: result has no {field}")))?;
        serde_json::from_value(value).map_err(|err| ApiError::Protocol(format!("{method}: {err}")))
    }

    pub fn ping(&self) -> Result<Pong, ApiError> {
        let result = self.call("ping", json!({}))?;
        serde_json::from_value(result).map_err(|err| ApiError::Protocol(format!("ping: {err}")))
    }

    pub fn session_snapshot(&self) -> Result<SessionSnapshot, ApiError> {
        self.call_field("session.snapshot", json!({}), "snapshot")
    }

    /// Closes one workspace. A linked worktree stays on disk.
    pub fn workspace_close(&self, workspace_id: &str) -> Result<(), ApiError> {
        self.call("workspace.close", json!({"workspace_id": workspace_id, "close_group": false})).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeServer;

    #[test]
    fn parse_response_returns_result_or_server_error() {
        let ok = parse_response(r#"{"id":"1","result":{"type":"pong","version":"0.9.3"}}"#).unwrap();
        assert_eq!(ok["version"], "0.9.3");
        let err = parse_response(r#"{"id":"1","error":{"code":"pane_not_found","message":"no pane w9"}}"#).unwrap_err();
        assert_eq!(err.code(), Some("pane_not_found"));
        assert_eq!(err.to_string(), "no pane w9");
    }

    #[test]
    fn parse_response_rejects_garbage_and_empty_envelopes() {
        assert!(matches!(parse_response("not json"), Err(ApiError::Protocol(_))));
        assert!(matches!(parse_response(r#"{"id":"1"}"#), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn a_server_error_without_details_still_reads_well() {
        let err = parse_response(r#"{"id":"1","error":{}}"#).unwrap_err();
        assert_eq!(err.code(), Some("error"));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn missing_socket_is_reported_as_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let api = Api::new(dir.path().join("herdr.sock"));
        assert!(matches!(api.ping(), Err(ApiError::Unavailable(_))));
    }

    #[test]
    fn each_call_sends_one_request_line_on_its_own_connection() {
        let server = FakeServer::start(|request| {
            let method = request["method"].as_str().unwrap().to_string();
            Some(json!({"type": "pong", "version": "0.9.3", "echo": method}))
        });
        let api = Api::new(server.path());
        assert_eq!(api.ping().unwrap().version, "0.9.3");
        assert_eq!(api.ping().unwrap().version, "0.9.3");
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "two calls must use two connections");
        assert_eq!(requests[0]["method"], "ping");
        assert_ne!(requests[0]["id"], requests[1]["id"], "request ids must be unique");
    }

    #[test]
    fn session_snapshot_decodes_the_snapshot_field() {
        let server = FakeServer::start(|_| {
            Some(json!({"type": "session_snapshot", "snapshot": {
                "version": "0.9.3",
                "workspaces": [{"workspace_id": "w1", "label": "repo"}],
                "agents": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "agent_status": "working"}]
            }}))
        });
        let snapshot = Api::new(server.path()).session_snapshot().unwrap();
        assert_eq!(snapshot.agents.len(), 1);
        assert_eq!(snapshot.workspaces[0].label, "repo");
    }

    #[test]
    fn workspace_close_never_closes_the_group() {
        let server = FakeServer::start(|_| Some(json!({"type": "ok"})));
        Api::new(server.path()).workspace_close("w7").unwrap();
        let request = &server.requests()[0];
        assert_eq!(request["method"], "workspace.close");
        assert_eq!(request["params"], json!({"workspace_id": "w7", "close_group": false}));
    }

    #[test]
    fn a_connection_closed_without_an_answer_is_a_protocol_error() {
        let server = FakeServer::start(|_| None);
        let err = Api::new(server.path()).ping().unwrap_err();
        assert!(matches!(err, ApiError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn a_silent_server_times_out_instead_of_hanging() {
        let server = FakeServer::silent();
        let api = Api::new(server.path()).with_timeout(Duration::from_millis(100));
        let started = std::time::Instant::now();
        assert!(matches!(api.ping(), Err(ApiError::Io(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
