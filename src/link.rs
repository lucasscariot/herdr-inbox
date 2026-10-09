//! Keeps the inbox in sync with one machine's Herdr server: connection state,
//! snapshots with the git facts of their checkouts, and live status events,
//! with reconnection.
//!
//! Herdr reports status transitions only through per-pane subscriptions, so
//! the link keeps one lifecycle subscription (panes and workspaces appearing,
//! closing, changing) and one status subscription covering every agent pane,
//! replaced whenever the set of panes changes.

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::app::{Connection, Input};
use crate::git::Checkout;
use crate::herdr::ApiError;
use crate::herdr::events::{self, Closer, Event};
use crate::herdr::transport::Transport;
use crate::herdr::types::SessionSnapshot;

/// Coalesces bursts of lifecycle events into one snapshot.
const DEBOUNCE: Duration = Duration::from_millis(120);
/// A full refresh even when nothing announced a change, as a safety net.
const RESYNC: Duration = Duration::from_secs(30);
/// How long a checkout's repository and branch are trusted before re-reading.
const CHECKOUT_TTL: Duration = Duration::from_secs(15);
const BACKOFF: [u64; 6] = [250, 500, 1000, 2000, 4000, 8000];

/// Requests to the link from the rest of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Refresh,
}

enum Signal {
    Request(Request),
    Lifecycle,
    Status(crate::herdr::types::AgentStatusChange),
    /// A subscription ended; `generation` says which one.
    Ended {
        generation: u64,
        error: Option<String>,
    },
}

pub struct Link {
    tx: Sender<Signal>,
}

impl Link {
    pub fn spawn(machine: String, transport: Transport, deliver: impl Fn(Input) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let link_tx = tx.clone();
        thread::spawn(move || {
            let mut link = Machine { id: machine, transport, checkouts: HashMap::new() };
            run(&mut link, link_tx, rx, deliver)
        });
        Self { tx }
    }

    pub fn request(&self, request: Request) {
        let _ = self.tx.send(Signal::Request(request));
    }
}

/// One machine's link state, owned by its thread.
struct Machine {
    id: String,
    transport: Transport,
    /// Repository and branch per checkout path, with when they were read.
    checkouts: HashMap<String, (Checkout, Instant)>,
}

impl Machine {
    fn connection(&self, connection: Connection) -> Input {
        Input::Connection { machine: self.id.clone(), connection }
    }

    /// Git facts for the snapshot's paths, re-reading only stale ones.
    fn checkouts_for(&mut self, snapshot: &SessionSnapshot) -> HashMap<String, Checkout> {
        let paths = crate::threads::checkout_paths(snapshot);
        let stale: Vec<String> = paths
            .iter()
            .filter(|path| self.checkouts.get(*path).is_none_or(|(_, at)| at.elapsed() > CHECKOUT_TTL))
            .cloned()
            .collect();
        if !stale.is_empty() {
            let now = Instant::now();
            let found = self.transport.checkouts(&stale);
            for path in &stale {
                match found.get(path) {
                    Some(checkout) => {
                        self.checkouts.insert(path.clone(), (checkout.clone(), now));
                    }
                    None => {
                        self.checkouts.remove(path);
                    }
                }
            }
        }
        self.checkouts.retain(|path, _| paths.contains(path));
        self.checkouts.iter().map(|(path, (checkout, _))| (path.clone(), checkout.clone())).collect()
    }
}

struct Subscriptions {
    lifecycle: Option<Closer>,
    status: Option<(Closer, BTreeSet<String>)>,
    generation: u64,
    status_generation: u64,
    lifecycle_generation: u64,
}

impl Subscriptions {
    fn close_all(&mut self) {
        if let Some(closer) = self.lifecycle.take() {
            closer.close();
        }
        if let Some((closer, _)) = self.status.take() {
            closer.close();
        }
    }
}

fn run(machine: &mut Machine, tx: Sender<Signal>, rx: Receiver<Signal>, deliver: impl Fn(Input)) {
    let mut attempt = 0usize;
    let mut ever_connected = false;
    let mut subs =
        Subscriptions { lifecycle: None, status: None, generation: 0, status_generation: 0, lifecycle_generation: 0 };
    loop {
        match connect(machine, &tx, &mut subs, &deliver) {
            Ok(()) => {
                ever_connected = true;
                attempt = 0;
                let reason = serve(machine, &tx, &rx, &mut subs, &deliver);
                subs.close_all();
                deliver(machine.connection(Connection::Lost(reason)));
            }
            Err(err) => {
                subs.close_all();
                // Only a local socket with nothing behind it means "no server";
                // a remote failure is always reported with SSH's reason.
                let connection = match err {
                    ApiError::Unavailable(_) if machine.transport.is_local() && !ever_connected => Connection::NoServer,
                    ApiError::Unavailable(_) if machine.transport.is_local() => {
                        Connection::Lost("the Herdr server stopped".into())
                    }
                    err => Connection::Lost(err.to_string()),
                };
                deliver(machine.connection(connection));
            }
        }
        let delay = Duration::from_millis(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
        attempt += 1;
        // Wait out the backoff, but stay responsive to explicit refreshes.
        match rx.recv_timeout(delay) {
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Pings, subscribes to lifecycle events, then takes the first snapshot and
/// subscribes to its panes' status.
fn connect(
    machine: &mut Machine,
    tx: &Sender<Signal>,
    subs: &mut Subscriptions,
    deliver: &impl Fn(Input),
) -> Result<(), ApiError> {
    machine.transport.call("ping", json!({}))?;
    subs.generation += 1;
    subs.lifecycle_generation = subs.generation;
    let lifecycle = machine.transport.subscribe(events::lifecycle_subscriptions())?;
    subs.lifecycle = Some(lifecycle.closer());
    pump(lifecycle, subs.lifecycle_generation, tx.clone());
    if !refresh(machine, tx, subs, deliver)? {
        let _ = tx.send(Signal::Request(Request::Refresh));
    }
    Ok(())
}

fn snapshot(machine: &Machine) -> Result<SessionSnapshot, ApiError> {
    let mut result = machine.transport.call("session.snapshot", json!({}))?;
    let value = result
        .get_mut("snapshot")
        .map(serde_json::Value::take)
        .ok_or_else(|| ApiError::Protocol("session.snapshot: result has no snapshot".into()))?;
    serde_json::from_value(value).map_err(|err| ApiError::Protocol(format!("session.snapshot: {err}")))
}

/// Takes a snapshot, re-subscribes to status if the panes changed, then
/// delivers the snapshot. Subscribing first means no transition between the
/// snapshot and the subscription is lost: Herdr sends each pane's current
/// status when a subscription starts.
///
/// Returns `Ok(false)` when the status subscription was refused (a pane closed
/// between the snapshot and the subscribe): the caller refreshes again soon.
fn refresh(
    machine: &mut Machine,
    tx: &Sender<Signal>,
    subs: &mut Subscriptions,
    deliver: &impl Fn(Input),
) -> Result<bool, ApiError> {
    let snapshot = snapshot(machine)?;
    let panes: BTreeSet<String> = snapshot.agents.iter().map(|a| a.pane_id.clone()).collect();
    let unchanged = subs.status.as_ref().is_some_and(|(_, current)| *current == panes);
    let mut subscribed = true;
    if !unchanged {
        let old = subs.status.take();
        if !panes.is_empty() {
            subs.generation += 1;
            subs.status_generation = subs.generation;
            match machine.transport.subscribe(events::status_subscriptions(panes.iter().map(String::as_str))) {
                Ok(status) => {
                    subs.status = Some((status.closer(), panes));
                    pump(status, subs.status_generation, tx.clone());
                }
                Err(ApiError::Server { .. }) => subscribed = false,
                Err(err) => return Err(err),
            }
        }
        if let Some((closer, _)) = old {
            closer.close();
        }
    }
    let checkouts = machine.checkouts_for(&snapshot);
    deliver(Input::Snapshot { machine: machine.id.clone(), snapshot, checkouts });
    Ok(subscribed)
}

/// Forwards a subscription's events to the link until it ends.
fn pump(mut subscription: events::Subscription, generation: u64, tx: Sender<Signal>) {
    thread::spawn(move || {
        let error = loop {
            match subscription.next_event() {
                Ok(Some(Event::Lifecycle(_))) => {
                    if tx.send(Signal::Lifecycle).is_err() {
                        return;
                    }
                }
                Ok(Some(Event::Status(change))) => {
                    if tx.send(Signal::Status(change)).is_err() {
                        return;
                    }
                }
                Ok(None) => break None,
                Err(err) => break Some(err.to_string()),
            }
        };
        let _ = tx.send(Signal::Ended { generation, error });
    });
}

/// Serves a live connection until it fails; returns why.
fn serve(
    machine: &mut Machine,
    tx: &Sender<Signal>,
    rx: &Receiver<Signal>,
    subs: &mut Subscriptions,
    deliver: &impl Fn(Input),
) -> String {
    let mut pending_since: Option<Instant> = None;
    let mut last_sync = Instant::now();
    loop {
        let wait = match pending_since {
            Some(since) => DEBOUNCE.saturating_sub(since.elapsed()),
            None => RESYNC.saturating_sub(last_sync.elapsed()),
        };
        match rx.recv_timeout(wait) {
            Ok(Signal::Request(Request::Refresh)) | Ok(Signal::Lifecycle) => {
                pending_since.get_or_insert_with(Instant::now);
            }
            Ok(Signal::Status(change)) => deliver(Input::Status { machine: machine.id.clone(), change }),
            Ok(Signal::Ended { generation, error }) => {
                if generation == subs.lifecycle_generation {
                    return error.unwrap_or_else(|| "Herdr closed the event stream".into());
                }
                if generation == subs.status_generation {
                    // The status stream died on its own (for example events
                    // were lost): resubscribe through a refresh.
                    subs.status = None;
                    pending_since.get_or_insert_with(Instant::now);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                pending_since = None;
                last_sync = Instant::now();
                match refresh(machine, tx, subs, deliver) {
                    Ok(true) => {}
                    Ok(false) => pending_since = Some(Instant::now()),
                    Err(err) => return err.to_string(),
                }
            }
            Err(RecvTimeoutError::Disconnected) => return "inbox is shutting down".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeServer, write_line};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    /// A fake server whose agents can change, and whose subscriptions can be
    /// fed events.
    struct World {
        agents: Mutex<Vec<Value>>,
        status_streams: Mutex<Vec<std::os::unix::net::UnixStream>>,
        lifecycle_streams: Mutex<Vec<std::os::unix::net::UnixStream>>,
    }

    fn agent(pane: &str, status: &str) -> Value {
        json!({"pane_id": pane, "workspace_id": pane.split(':').next().unwrap(), "tab_id": "t", "agent_status": status})
    }

    fn server(world: Arc<World>) -> FakeServer {
        FakeServer::start_raw(move |request, stream| {
            let id = &request["id"];
            match request["method"].as_str().unwrap_or("") {
                "ping" => write_line(stream, &json!({"id": id, "result": {"type": "pong", "version": "0.9.3"}})),
                "session.snapshot" => {
                    let agents = world.agents.lock().unwrap().clone();
                    write_line(
                        stream,
                        &json!({"id": id, "result": {"type": "session_snapshot", "snapshot": {"agents": agents}}}),
                    );
                }
                "events.subscribe" => {
                    write_line(stream, &json!({"id": id, "result": {"type": "subscription_started"}}));
                    let subs = request["params"]["subscriptions"].as_array().unwrap();
                    let keep = stream.try_clone().unwrap();
                    if subs[0]["type"] == "pane.agent_status_changed" {
                        world.status_streams.lock().unwrap().push(keep);
                    } else {
                        world.lifecycle_streams.lock().unwrap().push(keep);
                    }
                    std::thread::sleep(Duration::from_secs(30));
                }
                _ => {}
            }
        })
    }

    fn spawn_local(socket: &std::path::Path, deliver: impl Fn(Input) + Send + 'static) -> Link {
        let transport = Transport::Local {
            socket: socket.to_path_buf(),
            herdr: crate::herdr::terminal::HerdrCommand::new("herdr"),
        };
        Link::spawn("local".into(), transport, deliver)
    }

    fn no_server() -> Input {
        Input::Connection { machine: "local".into(), connection: Connection::NoServer }
    }

    fn inputs() -> (impl Fn(Input) + Send + 'static, Receiver<Input>) {
        let (tx, rx) = mpsc::channel();
        (
            move |input| {
                let _ = tx.send(input);
            },
            rx,
        )
    }

    fn next_snapshot(rx: &Receiver<Input>) -> Vec<String> {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).expect("an input") {
                Input::Snapshot { snapshot, .. } => return snapshot.agents.into_iter().map(|a| a.pane_id).collect(),
                _ => continue,
            }
        }
    }

    #[test]
    fn without_a_server_the_link_reports_no_server_and_keeps_trying() {
        let dir = tempfile::tempdir().unwrap();
        let (deliver, rx) = inputs();
        let _link = spawn_local(&dir.path().join("herdr.sock"), deliver);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), no_server());
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), no_server());
    }

    #[test]
    fn connecting_delivers_a_snapshot_and_subscribes_to_every_agent_pane() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle"), agent("w2:p1", "working")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let _link = spawn_local(server.path(), deliver);
        assert_eq!(next_snapshot(&rx), ["w1:p1", "w2:p1"]);
        let subscribes: Vec<Value> =
            server.requests().into_iter().filter(|r| r["method"] == "events.subscribe").collect();
        assert_eq!(subscribes.len(), 2, "one lifecycle and one status subscription");
        let status = &subscribes[1]["params"]["subscriptions"];
        assert_eq!(
            status,
            &json!([
                {"type": "pane.agent_status_changed", "pane_id": "w1:p1"},
                {"type": "pane.agent_status_changed", "pane_id": "w2:p1"},
            ])
        );
    }

    #[test]
    fn status_events_are_forwarded_and_lifecycle_events_trigger_one_snapshot() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let _link = spawn_local(server.path(), deliver);
        next_snapshot(&rx);
        thread::sleep(Duration::from_millis(50));
        let mut status = world.status_streams.lock().unwrap()[0].try_clone().unwrap();
        write_line(
            &mut status,
            &json!({"event": "pane.agent_status_changed", "data": {"pane_id": "w1:p1", "agent_status": "blocked"}}),
        );
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Input::Status { machine, change } => {
                assert_eq!(machine, "local");
                assert_eq!(change.pane_id, "w1:p1");
            }
            other => panic!("expected a status change, got {other:?}"),
        }
        world.agents.lock().unwrap().push(agent("w3:p1", "working"));
        let mut lifecycle = world.lifecycle_streams.lock().unwrap()[0].try_clone().unwrap();
        for _ in 0..5 {
            write_line(&mut lifecycle, &json!({"event": "pane_created", "data": {}}));
        }
        assert_eq!(next_snapshot(&rx), ["w1:p1", "w3:p1"]);
        thread::sleep(DEBOUNCE * 3);
        let snapshots = server.requests().iter().filter(|r| r["method"] == "session.snapshot").count();
        assert_eq!(snapshots, 2, "a burst of five events costs one snapshot");
        let subscribes = server.requests().iter().filter(|r| r["method"] == "events.subscribe").count();
        assert_eq!(subscribes, 3, "the new pane set gets a new status subscription");
    }

    #[test]
    fn an_unchanged_pane_set_keeps_its_status_subscription() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let link = spawn_local(server.path(), deliver);
        next_snapshot(&rx);
        link.request(Request::Refresh);
        next_snapshot(&rx);
        let subscribes = server.requests().iter().filter(|r| r["method"] == "events.subscribe").count();
        assert_eq!(subscribes, 2);
    }

    #[test]
    fn no_agents_means_no_status_subscription() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let _link = spawn_local(server.path(), deliver);
        assert!(next_snapshot(&rx).is_empty());
        let subscribes = server.requests().iter().filter(|r| r["method"] == "events.subscribe").count();
        assert_eq!(subscribes, 1, "an empty subscription list would be rejected");
    }

    #[test]
    fn a_dropped_lifecycle_stream_reports_lost_and_reconnects() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let _link = spawn_local(server.path(), deliver);
        next_snapshot(&rx);
        thread::sleep(Duration::from_millis(50));
        let lifecycle = world.lifecycle_streams.lock().unwrap().remove(0);
        lifecycle.shutdown(std::net::Shutdown::Both).unwrap();
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Input::Connection { connection: Connection::Lost(_), .. } => {}
            other => panic!("expected lost, got {other:?}"),
        }
        assert_eq!(next_snapshot(&rx), ["w1:p1"], "the link reconnects on its own");
    }

    #[test]
    fn a_refused_status_subscription_is_retried_without_dropping_the_connection() {
        let refused = Arc::new(Mutex::new(false));
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let inner = server(Arc::clone(&world));
        let target = inner.path().to_path_buf();
        let flag = Arc::clone(&refused);
        // A proxy that refuses the first status subscription, as Herdr does
        // when a pane closed in between.
        let proxy = FakeServer::start_raw(move |request, stream| {
            let is_status = request["params"]["subscriptions"][0]["type"] == "pane.agent_status_changed";
            if is_status && !std::mem::replace(&mut *flag.lock().unwrap(), true) {
                write_line(
                    stream,
                    &json!({"id": request["id"], "error": {"code": "pane_not_found", "message": "gone"}}),
                );
                return;
            }
            let mut upstream = std::os::unix::net::UnixStream::connect(&target).unwrap();
            write_line(&mut upstream, request);
            let mut down = stream.try_clone().unwrap();
            let _ = std::io::copy(&mut upstream, &mut down);
        });
        let (deliver, rx) = inputs();
        let _link = spawn_local(proxy.path(), deliver);
        assert_eq!(next_snapshot(&rx), ["w1:p1"]);
        assert_eq!(next_snapshot(&rx), ["w1:p1"], "a quick refresh follows the refusal");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(world.status_streams.lock().unwrap().len(), 1, "the retry subscribed");
        assert!(rx.try_iter().all(|input| !matches!(input, Input::Connection { connection: Connection::Lost(_), .. })));
    }

    #[test]
    fn a_dead_status_stream_is_replaced_without_losing_the_connection() {
        let world = Arc::new(World {
            agents: Mutex::new(vec![agent("w1:p1", "idle")]),
            status_streams: Mutex::default(),
            lifecycle_streams: Mutex::default(),
        });
        let server = server(Arc::clone(&world));
        let (deliver, rx) = inputs();
        let _link = spawn_local(server.path(), deliver);
        next_snapshot(&rx);
        thread::sleep(Duration::from_millis(50));
        let mut status = world.status_streams.lock().unwrap().remove(0);
        write_line(&mut status, &json!({"error": {"code": "events_lost", "message": "slow reader"}}));
        status.shutdown(std::net::Shutdown::Both).unwrap();
        assert_eq!(next_snapshot(&rx), ["w1:p1"]);
        let subscribes = server.requests().iter().filter(|r| r["method"] == "events.subscribe").count();
        assert_eq!(subscribes, 3);
        assert!(rx.try_iter().all(|input| !matches!(input, Input::Connection { connection: Connection::Lost(_), .. })));
    }
}
