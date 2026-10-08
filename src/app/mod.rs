//! The inbox's state machine. `App::update` turns one input into state changes
//! and a list of effects for the runtime to perform; nothing in here does I/O,
//! so every behaviour is testable without a server or a terminal.

mod input;
mod layout;

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};

use crate::herdr::terminal::{Control, Message};
use crate::herdr::types::{AgentStatus, AgentStatusChange, SessionSnapshot};
use crate::screen::Screen;
use crate::threads::{self, Activity, Thread, ThreadId};

pub use layout::{Layout, Row, RowKind};

const NOTICE_TTL: Duration = Duration::from_secs(4);
/// How long a server the user asked for may take to answer.
const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Live,
    /// No server answers on the socket.
    NoServer,
    /// The server answered once but the link is down; the runtime retries.
    Lost(String),
    /// A server is being started on the user's request, since the given time.
    Starting(SystemTime),
}

/// Herdr's close reason when another client takes a pane over.
pub const TAKEN_OVER: &str = "terminal attach taken over";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamState {
    Attaching,
    Live,
    /// The stream ended, with Herdr's reason when it gave one.
    Closed {
        reason: Option<String>,
    },
}

pub struct OpenThread {
    pub id: ThreadId,
    pub pane_id: String,
    /// Identifies this attachment; messages from older ones are ignored.
    pub generation: u64,
    pub screen: Screen,
    pub stream: StreamState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
    pub until: SystemTime,
}

/// Work for the runtime. Effects are performed in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Release the current terminal session, if any.
    Detach,
    Attach {
        generation: u64,
        pane_id: String,
        cols: u16,
        rows: u16,
    },
    /// Forward to the session of `generation`.
    Send {
        generation: u64,
        control: Control,
    },
    Archive {
        thread: ThreadId,
        workspace_id: String,
        title: String,
    },
    StartServer,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Snapshot(SessionSnapshot),
    Status(AgentStatusChange),
    Connection(Connection),
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Resize { width: u16, height: u16 },
    Terminal { generation: u64, message: Message },
    Archived { title: String, result: Result<(), String> },
    Tick,
}

/// Finds the git checkout of a path; injected so tests need no repositories.
pub type CheckoutReader = Box<dyn Fn(&std::path::Path) -> Option<crate::git::Checkout>>;

pub struct App {
    pub threads: Vec<Thread>,
    /// The highlighted thread in the list, tracked by identity so it survives
    /// reordering.
    pub cursor: Option<ThreadId>,
    pub open: Option<OpenThread>,
    pub focus: Focus,
    pub connection: Connection,
    pub notice: Option<Notice>,
    /// A thread waiting for the user to confirm archiving it.
    pub confirm_archive: Option<ThreadId>,
    pub layout: Layout,
    activity: Activity,
    snapshot: Option<SessionSnapshot>,
    next_generation: u64,
    checkout: CheckoutReader,
}

impl App {
    pub fn new(width: u16, height: u16, checkout: CheckoutReader) -> Self {
        Self {
            threads: Vec::new(),
            cursor: None,
            open: None,
            focus: Focus::List,
            connection: Connection::Connecting,
            notice: None,
            confirm_archive: None,
            layout: Layout::new(width, height),
            activity: Activity::default(),
            snapshot: None,
            next_generation: 1,
            checkout,
        }
    }

    pub fn update(&mut self, input: Input, now: SystemTime) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Snapshot(snapshot) => self.on_snapshot(snapshot, now, &mut effects),
            Input::Status(change) => self.on_status(change, now),
            Input::Connection(connection) => self.on_connection(connection, now, &mut effects),
            Input::Key(key) => input::key(self, key, now, &mut effects),
            Input::Paste(text) => input::paste(self, &text, &mut effects),
            Input::Mouse(mouse) => input::mouse(self, mouse, now, &mut effects),
            Input::Resize { width, height } => self.on_resize(width, height, &mut effects),
            Input::Terminal { generation, message } => self.on_terminal(generation, message),
            Input::Archived { title, result } => match result {
                Ok(()) => self.notify(format!("Archived “{title}”"), NoticeKind::Info, now),
                Err(err) => self.notify(format!("Could not archive “{title}”: {err}"), NoticeKind::Error, now),
            },
            Input::Tick => {
                if self.notice.as_ref().is_some_and(|n| n.until <= now) {
                    self.notice = None;
                }
            }
        }
        self.layout.update(&self.threads, self.cursor.as_deref());
        effects
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn cursor_index(&self) -> Option<usize> {
        let cursor = self.cursor.as_deref()?;
        self.threads.iter().position(|t| t.id == cursor)
    }

    pub fn notify(&mut self, text: impl Into<String>, kind: NoticeKind, now: SystemTime) {
        self.notice = Some(Notice { text: text.into(), kind, until: now + NOTICE_TTL });
    }

    fn on_connection(&mut self, connection: Connection, now: SystemTime, effects: &mut Vec<Effect>) {
        // While a requested server boots, "no server yet" is expected.
        if let (Connection::Starting(since), Connection::NoServer) = (&self.connection, &connection) {
            if now.duration_since(*since).unwrap_or_default() < START_TIMEOUT {
                return;
            }
            self.notify("Herdr did not start. Try `herdr server` in a shell to see why.", NoticeKind::Error, now);
        }
        let lost = !matches!(connection, Connection::Live | Connection::Connecting);
        self.connection = connection;
        if lost {
            // The server is gone: its threads and streams are stale.
            if self.open.take().is_some() {
                effects.push(Effect::Detach);
            }
            self.threads.clear();
            self.snapshot = None;
            self.cursor = None;
            self.confirm_archive = None;
            self.focus = Focus::List;
        }
    }

    fn on_snapshot(&mut self, snapshot: SessionSnapshot, now: SystemTime, effects: &mut Vec<Effect>) {
        let first = self.snapshot.is_none();
        self.connection = Connection::Live;
        for agent in &snapshot.agents {
            if first || self.activity.knows(&agent.pane_id) {
                self.activity.observe(&agent.pane_id, agent.agent_status, now);
            } else {
                self.activity.appear(&agent.pane_id, agent.agent_status, now);
            }
        }
        let live: HashSet<ThreadId> = snapshot.agents.iter().map(|a| a.pane_id.clone()).collect();
        self.activity.retain(&live);
        self.snapshot = Some(snapshot);
        self.rebuild(now);

        if let Some(open) = &self.open
            && !live.contains(&open.id)
        {
            self.open = None;
            effects.push(Effect::Detach);
            if self.focus == Focus::Terminal {
                self.focus = Focus::List;
            }
            self.notify("The thread you had open ended", NoticeKind::Info, now);
        }
        if self.confirm_archive.as_ref().is_some_and(|id| !live.contains(id)) {
            self.confirm_archive = None;
        }
        self.fix_cursor();
        if first
            && self.open.is_none()
            && let Some(id) = self.cursor.clone()
        {
            self.open_thread(&id, now, effects);
        }
    }

    fn on_status(&mut self, change: AgentStatusChange, now: SystemTime) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let Some(agent) = snapshot.agents.iter_mut().find(|a| a.pane_id == change.pane_id) else {
            return;
        };
        agent.agent_status = change.agent_status;
        if change.agent.is_some() {
            agent.agent = change.agent;
        }
        if change.display_agent.is_some() {
            agent.display_agent = change.display_agent;
        }
        if change.title.is_some() {
            agent.title = change.title;
        }
        self.activity.observe(&change.pane_id, change.agent_status, now);
        // Finishing while the user watches it is not news.
        let watching = self.focus == Focus::Terminal && self.open.as_ref().is_some_and(|o| o.id == change.pane_id);
        if watching && change.agent_status == AgentStatus::Done {
            self.activity.mark_seen(&change.pane_id);
        }
        self.rebuild(now);
    }

    fn rebuild(&mut self, _now: SystemTime) {
        if let Some(snapshot) = &self.snapshot {
            self.threads = threads::build(snapshot, &self.activity, self.checkout.as_ref());
        }
    }

    /// Keeps the cursor on an existing thread: its old position's neighbour if
    /// it vanished, the first thread if there was none.
    fn fix_cursor(&mut self) {
        if self.cursor.as_deref().is_some_and(|id| self.thread(id).is_some()) {
            return;
        }
        let previous = self.layout.cursor_index();
        self.cursor = match previous {
            Some(index) if !self.threads.is_empty() => Some(self.threads[index.min(self.threads.len() - 1)].id.clone()),
            _ => self.threads.first().map(|t| t.id.clone()),
        };
    }

    fn on_resize(&mut self, width: u16, height: u16, effects: &mut Vec<Effect>) {
        let before = self.layout.terminal_size();
        self.layout.resize(width, height);
        let after = self.layout.terminal_size();
        if before != after
            && let Some(open) = &self.open
        {
            effects.push(Effect::Send {
                generation: open.generation,
                control: Control::Resize { cols: after.0, rows: after.1 },
            });
        }
    }

    fn on_terminal(&mut self, generation: u64, message: Message) {
        let Some(open) = self.open.as_mut().filter(|o| o.generation == generation) else {
            return;
        };
        match message {
            Message::Frame(frame) => {
                open.screen.apply(&frame);
                open.stream = StreamState::Live;
            }
            Message::Closed { reason } => open.stream = StreamState::Closed { reason },
        }
    }

    /// Shows a thread in the terminal area, attaching a fresh session.
    pub(crate) fn open_thread(&mut self, id: &str, now: SystemTime, effects: &mut Vec<Effect>) {
        let Some(thread) = self.thread(id) else {
            return;
        };
        let pane_id = thread.pane_id.clone();
        // Re-opening the open thread is a no-op, unless its stream ended (for
        // instance because another client took it over): then it re-attaches.
        let already_streaming =
            self.open.as_ref().is_some_and(|o| o.id == id && !matches!(o.stream, StreamState::Closed { .. }));
        if already_streaming {
            self.mark_seen(id, now);
            return;
        }
        if self.open.is_some() {
            effects.push(Effect::Detach);
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        let (cols, rows) = self.layout.terminal_size();
        self.open = Some(OpenThread {
            id: id.to_string(),
            pane_id: pane_id.clone(),
            generation,
            screen: Screen::new(cols, rows),
            stream: StreamState::Attaching,
        });
        effects.push(Effect::Attach { generation, pane_id, cols, rows });
        self.mark_seen(id, now);
    }

    fn mark_seen(&mut self, id: &str, now: SystemTime) {
        self.activity.mark_seen(id);
        self.rebuild(now);
    }
}

#[cfg(test)]
mod tests;
