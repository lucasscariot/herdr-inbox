//! `events.subscribe`: a long-lived connection that streams one JSON event per
//! line after a `subscription_started` acknowledgement.
//!
//! Herdr resets a subscription connection that receives any other request, so
//! each subscription owns its connection and nothing else is sent on it.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::api::{ApiError, connect, next_request_id, parse_response};
use super::types::AgentStatusChange;

/// Lifecycle changes that can add, remove or relabel a thread.
pub const LIFECYCLE: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "worktree.created",
    "worktree.removed",
    "tab.renamed",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An agent's status, title or agent label changed.
    Status(AgentStatusChange),
    /// Something in the session's structure changed; the name is normalized
    /// to Herdr's dotted form (`pane.closed`).
    Lifecycle(String),
}

pub fn lifecycle_subscriptions() -> Vec<Value> {
    LIFECYCLE.iter().map(|kind| json!({"type": kind})).collect()
}

pub fn status_subscriptions<'a>(pane_ids: impl IntoIterator<Item = &'a str>) -> Vec<Value> {
    pane_ids.into_iter().map(|pane_id| json!({"type": "pane.agent_status_changed", "pane_id": pane_id})).collect()
}

/// Herdr sends lifecycle names with underscores (`pane_closed`) and
/// subscription events with dots (`pane.agent_status_changed`).
fn normalize_name(name: &str) -> String {
    match name.split_once('_') {
        Some((domain, rest)) if !name.contains('.') => format!("{domain}.{rest}"),
        _ => name.to_string(),
    }
}

pub fn parse_event(line: &str) -> Result<Option<Event>, ApiError> {
    let value: Value =
        serde_json::from_str(line.trim()).map_err(|err| ApiError::Protocol(format!("invalid event JSON ({err})")))?;
    if value.get("error").is_some() {
        return parse_response(line).map(|_| None);
    }
    let Some(name) = value.get("event").and_then(Value::as_str) else {
        return Ok(None);
    };
    let name = normalize_name(name);
    if name == "pane.agent_status_changed" {
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let change = serde_json::from_value(data).map_err(|err| ApiError::Protocol(format!("{name}: {err}")))?;
        return Ok(Some(Event::Status(change)));
    }
    Ok(Some(Event::Lifecycle(name)))
}

pub struct Subscription {
    reader: Box<dyn BufRead + Send>,
    closer: Closer,
}

/// Ends a subscription from another thread; its reader then sees the end of
/// the stream.
#[derive(Debug, Clone)]
pub struct Closer(CloseHandle);

#[derive(Debug, Clone)]
enum CloseHandle {
    Socket(Arc<UnixStream>),
    Process(Arc<Mutex<Child>>),
}

impl Closer {
    pub fn socket(stream: UnixStream) -> Self {
        Self(CloseHandle::Socket(Arc::new(stream)))
    }

    pub fn process(child: Arc<Mutex<Child>>) -> Self {
        Self(CloseHandle::Process(child))
    }

    pub fn close(&self) {
        match &self.0 {
            CloseHandle::Socket(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            CloseHandle::Process(child) => {
                let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// The `events.subscribe` request line.
pub fn subscribe_request(subscriptions: Vec<Value>) -> Result<Vec<u8>, ApiError> {
    let request = json!({
        "id": next_request_id(),
        "method": "events.subscribe",
        "params": {"subscriptions": subscriptions},
    });
    let mut line = serde_json::to_vec(&request).map_err(|err| ApiError::Protocol(err.to_string()))?;
    line.push(b'\n');
    Ok(line)
}

/// Reads the `subscription_started` acknowledgement.
pub fn read_ack(reader: &mut dyn BufRead) -> Result<(), ApiError> {
    let mut ack = String::new();
    if reader.read_line(&mut ack)? == 0 {
        return Err(ApiError::Protocol("subscription closed before it started".into()));
    }
    let result = parse_response(&ack)?;
    if result.get("type").and_then(Value::as_str) != Some("subscription_started") {
        return Err(ApiError::Protocol(format!("subscription not acknowledged: {result}")));
    }
    Ok(())
}

impl Subscription {
    /// Subscribes on a local Unix socket.
    pub fn open(socket: &Path, subscriptions: Vec<Value>) -> Result<Self, ApiError> {
        let mut stream = connect(socket)?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        // The acknowledgement must come quickly; events afterwards may not.
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(&subscribe_request(subscriptions)?)?;
        let control = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        read_ack(&mut reader)?;
        // macOS refuses socket options once the peer has closed (EINVAL); the
        // next read then reports the end of the stream, so this cannot fail
        // in a way that matters.
        let _ = reader.get_ref().set_read_timeout(None);
        Ok(Self { reader: Box::new(reader), closer: Closer::socket(control) })
    }

    /// A subscription whose acknowledgement was already read from `reader`.
    pub fn from_parts(reader: Box<dyn BufRead + Send>, closer: Closer) -> Self {
        Self { reader, closer }
    }

    pub fn closer(&self) -> Closer {
        self.closer.clone()
    }

    /// The next event, or `None` once the server closes the stream. Lines that
    /// carry no event are skipped.
    pub fn next_event(&mut self) -> Result<Option<Event>, ApiError> {
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            if let Some(event) = parse_event(&line)? {
                return Ok(Some(event));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::types::AgentStatus;
    use crate::testing::{FakeServer, write_line};

    #[test]
    fn lifecycle_names_are_normalized_to_dotted_form() {
        let event = parse_event(r#"{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p1"}}"#);
        assert_eq!(event.unwrap(), Some(Event::Lifecycle("pane.closed".into())));
        let event = parse_event(r#"{"event":"pane_agent_detected","data":{}}"#);
        assert_eq!(event.unwrap(), Some(Event::Lifecycle("pane.agent_detected".into())));
        let event = parse_event(r#"{"event":"workspace.closed","data":{}}"#);
        assert_eq!(event.unwrap(), Some(Event::Lifecycle("workspace.closed".into())));
    }

    #[test]
    fn status_events_carry_the_change() {
        let line = r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p2","workspace_id":"w1","agent_status":"blocked","agent":"codex","title":"Fix it","state_labels":{}}}"#;
        let Some(Event::Status(change)) = parse_event(line).unwrap() else {
            panic!("expected a status event");
        };
        assert_eq!(change.pane_id, "w1:p2");
        assert_eq!(change.agent_status, AgentStatus::Blocked);
        assert_eq!(change.title.as_deref(), Some("Fix it"));
    }

    #[test]
    fn the_underscore_spelling_of_status_events_is_also_a_status_event() {
        let line = r#"{"event":"pane_agent_status_changed","data":{"pane_id":"w1:p2","agent_status":"done"}}"#;
        assert!(matches!(parse_event(line).unwrap(), Some(Event::Status(_))));
    }

    #[test]
    fn an_error_line_ends_the_stream_with_its_code() {
        let err = parse_event(r#"{"error":{"code":"events_lost","message":"too slow"}}"#).unwrap_err();
        assert_eq!(err.code(), Some("events_lost"));
    }

    #[test]
    fn lines_without_an_event_are_ignored() {
        assert_eq!(parse_event(r#"{"type":"heartbeat"}"#).unwrap(), None);
    }

    #[test]
    fn subscription_lists_are_well_formed() {
        let lifecycle = lifecycle_subscriptions();
        assert_eq!(lifecycle.len(), LIFECYCLE.len());
        assert!(lifecycle.iter().all(|entry| entry.as_object().unwrap().len() == 1));
        let status = status_subscriptions(["w1:p1", "w2:p1"]);
        assert_eq!(status[1], json!({"type": "pane.agent_status_changed", "pane_id": "w2:p1"}));
    }

    #[test]
    fn every_lifecycle_kind_is_one_herdr_accepts() {
        // Herdr rejects the whole subscription when one entry is invalid, so the
        // list must stay within the schema's Subscription enum (Herdr 0.9.3).
        const ACCEPTED: &[&str] = &[
            "workspace.created",
            "workspace.updated",
            "workspace.metadata_updated",
            "workspace.renamed",
            "workspace.moved",
            "workspace.reordered",
            "workspace.closed",
            "workspace.focused",
            "worktree.created",
            "worktree.opened",
            "worktree.removed",
            "tab.created",
            "tab.closed",
            "tab.focused",
            "tab.renamed",
            "tab.moved",
            "pane.created",
            "pane.closed",
            "pane.updated",
            "pane.focused",
            "pane.moved",
            "pane.exited",
            "pane.agent_detected",
            "layout.updated",
        ];
        for kind in LIFECYCLE {
            assert!(ACCEPTED.contains(kind), "{kind} is not a Herdr subscription type");
        }
    }

    #[test]
    fn a_subscription_streams_events_after_the_acknowledgement() {
        let server = FakeServer::start_raw(|request, stream| {
            write_line(stream, &json!({"id": request["id"], "result": {"type": "subscription_started"}}));
            write_line(stream, &json!({"event": "pane_created", "data": {}}));
            write_line(
                stream,
                &json!({"event": "pane.agent_status_changed", "data": {"pane_id": "w1:p1", "agent_status": "working"}}),
            );
        });
        let mut subscription = Subscription::open(server.path(), lifecycle_subscriptions()).unwrap();
        assert_eq!(subscription.next_event().unwrap(), Some(Event::Lifecycle("pane.created".into())));
        assert!(matches!(subscription.next_event().unwrap(), Some(Event::Status(_))));
        assert_eq!(subscription.next_event().unwrap(), None, "server closed the stream");
        let request = &server.requests()[0];
        assert_eq!(request["method"], "events.subscribe");
        assert_eq!(request["params"]["subscriptions"].as_array().unwrap().len(), LIFECYCLE.len());
    }

    #[test]
    fn a_rejected_subscription_reports_the_server_error() {
        let server = FakeServer::start_raw(|request, stream| {
            write_line(
                stream,
                &json!({"id": request["id"], "error": {"code": "pane_not_found", "message": "no such pane"}}),
            );
        });
        let err = Subscription::open(server.path(), status_subscriptions(["w9:p9"])).err().unwrap();
        assert_eq!(err.code(), Some("pane_not_found"));
    }

    #[test]
    fn an_unexpected_acknowledgement_is_rejected() {
        let server = FakeServer::start_raw(|request, stream| {
            write_line(stream, &json!({"id": request["id"], "result": {"type": "ok"}}));
        });
        let result = Subscription::open(server.path(), lifecycle_subscriptions());
        assert!(matches!(result, Err(ApiError::Protocol(_))), "{:?}", result.err());
    }

    #[test]
    fn a_closer_ends_a_blocked_reader() {
        let server = FakeServer::start_raw(|request, stream| {
            write_line(stream, &json!({"id": request["id"], "result": {"type": "subscription_started"}}));
            std::thread::sleep(Duration::from_secs(5));
        });
        let mut subscription = Subscription::open(server.path(), lifecycle_subscriptions()).unwrap();
        let closer = subscription.closer();
        let reader = std::thread::spawn(move || subscription.next_event());
        std::thread::sleep(Duration::from_millis(50));
        closer.close();
        let started = std::time::Instant::now();
        let result = reader.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(result, Ok(None) | Err(_)));
    }
}
