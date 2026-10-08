//! The inbox's state machine. `App::update` turns one input into state changes
//! and a list of effects for the runtime to perform; nothing in here does I/O,
//! so every behaviour is testable without a server or a terminal.

mod input;
mod layout;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};

use crate::git::Checkout;
use crate::herdr::terminal::{Control, Message};
use crate::herdr::types::{AgentStatus, AgentStatusChange, SessionSnapshot};
use crate::screen::Screen;
use crate::threads::{self, Activity, Source, Thread, ThreadId};

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
    /// No server answers on the local socket.
    NoServer,
    /// The link is down; it retries on its own. The text says why.
    Lost(String),
    /// A local server is being started on the user's request, since then.
    Starting(SystemTime),
}

impl Connection {
    pub fn is_live(&self) -> bool {
        matches!(self, Connection::Live)
    }
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
    pub machine: String,
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

/// A machine the inbox shows threads from. The first one is always local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineInfo {
    pub id: String,
    pub label: String,
}

impl MachineInfo {
    pub fn local() -> Self {
        Self { id: threads::LOCAL.into(), label: "Local".into() }
    }
}

#[derive(Debug, Clone)]
pub struct MachineState {
    pub id: String,
    pub label: String,
    pub connection: Connection,
    snapshot: Option<SessionSnapshot>,
    checkouts: HashMap<String, Checkout>,
}

impl MachineState {
    pub fn is_local(&self) -> bool {
        self.id == threads::LOCAL
    }
}

/// Work for the runtime. Effects are performed in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Release the current terminal session, if any.
    Detach,
    Attach {
        generation: u64,
        machine: String,
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
        machine: String,
        thread: ThreadId,
        workspace_id: String,
        title: String,
    },
    /// Start the local Herdr server.
    StartServer,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Snapshot { machine: String, snapshot: SessionSnapshot, checkouts: HashMap<String, Checkout> },
    Status { machine: String, change: AgentStatusChange },
    Connection { machine: String, connection: Connection },
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Resize { width: u16, height: u16 },
    Terminal { generation: u64, message: Message },
    Archived { title: String, result: Result<(), String> },
    Tick,
}

pub struct App {
    pub threads: Vec<Thread>,
    pub machines: Vec<MachineState>,
    /// The highlighted thread in the list, tracked by identity so it survives
    /// reordering.
    pub cursor: Option<ThreadId>,
    pub open: Option<OpenThread>,
    pub focus: Focus,
    pub notice: Option<Notice>,
    /// A thread waiting for the user to confirm archiving it.
    pub confirm_archive: Option<ThreadId>,
    pub layout: Layout,
    activity: Activity,
    next_generation: u64,
    /// Whether the first thread was opened on its own already.
    auto_opened: bool,
    /// The thread that was open when its machine dropped, to re-open when the
    /// machine comes back.
    resume: Option<ThreadId>,
}

impl App {
    /// `machines` lists the saved remote machines; the local one is added first.
    pub fn new(width: u16, height: u16, machines: Vec<MachineInfo>) -> Self {
        let mut all = vec![MachineInfo::local()];
        all.extend(machines.into_iter().filter(|m| m.id != threads::LOCAL));
        Self {
            threads: Vec::new(),
            machines: all
                .into_iter()
                .map(|m| MachineState {
                    id: m.id,
                    label: m.label,
                    connection: Connection::Connecting,
                    snapshot: None,
                    checkouts: HashMap::new(),
                })
                .collect(),
            cursor: None,
            open: None,
            focus: Focus::List,
            notice: None,
            confirm_archive: None,
            layout: Layout::new(width, height),
            activity: Activity::default(),
            next_generation: 1,
            auto_opened: false,
            resume: None,
        }
    }

    pub fn update(&mut self, input: Input, now: SystemTime) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Snapshot { machine, snapshot, checkouts } => {
                self.on_snapshot(&machine, snapshot, checkouts, now, &mut effects)
            }
            Input::Status { machine, change } => self.on_status(&machine, change, now),
            Input::Connection { machine, connection } => self.on_connection(&machine, connection, now, &mut effects),
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

    pub fn machine(&self, id: &str) -> Option<&MachineState> {
        self.machines.iter().find(|m| m.id == id)
    }

    pub fn local(&self) -> &MachineState {
        &self.machines[0]
    }

    /// Without saved machines, a missing local server is the whole screen.
    pub fn local_only(&self) -> bool {
        self.machines.len() == 1
    }

    pub fn needs_server_screen(&self) -> bool {
        self.local_only() && matches!(self.local().connection, Connection::NoServer | Connection::Starting(_))
    }

    /// The connection the UI describes when it needs one word for all
    /// machines: the local one alone, or live as soon as any machine is.
    pub fn overall_connection(&self) -> Connection {
        if self.local_only() {
            return self.local().connection.clone();
        }
        if self.machines.iter().any(|m| m.connection.is_live()) {
            return Connection::Live;
        }
        if self.machines.iter().any(|m| m.connection == Connection::Connecting) {
            return Connection::Connecting;
        }
        Connection::Lost("no machine is reachable".into())
    }

    pub fn notify(&mut self, text: impl Into<String>, kind: NoticeKind, now: SystemTime) {
        self.notice = Some(Notice { text: text.into(), kind, until: now + NOTICE_TTL });
    }

    fn machine_mut(&mut self, id: &str) -> Option<&mut MachineState> {
        self.machines.iter_mut().find(|m| m.id == id)
    }

    fn on_connection(&mut self, id: &str, connection: Connection, now: SystemTime, effects: &mut Vec<Effect>) {
        let Some(machine) = self.machine_mut(id) else {
            return;
        };
        // While a requested server boots, "no server yet" is expected.
        if let (Connection::Starting(since), Connection::NoServer) = (&machine.connection, &connection) {
            if now.duration_since(*since).unwrap_or_default() < START_TIMEOUT {
                return;
            }
            self.notify("Herdr did not start. Try `herdr server` in a shell to see why.", NoticeKind::Error, now);
        }
        let Some(machine) = self.machine_mut(id) else {
            return;
        };
        let lost = !matches!(connection, Connection::Live | Connection::Connecting);
        machine.connection = connection;
        if !lost {
            return;
        }
        // The machine's threads and streams are stale.
        machine.snapshot = None;
        machine.checkouts.clear();
        if let Some(open) = self.open.take_if(|o| o.machine == id) {
            self.resume = Some(open.id);
            effects.push(Effect::Detach);
            self.focus = Focus::List;
        }
        self.rebuild();
        if self.confirm_archive.as_deref().is_some_and(|c| self.thread(c).is_none()) {
            self.confirm_archive = None;
        }
        self.fix_cursor();
    }

    fn on_snapshot(
        &mut self,
        id: &str,
        snapshot: SessionSnapshot,
        checkouts: HashMap<String, Checkout>,
        now: SystemTime,
        effects: &mut Vec<Effect>,
    ) {
        let Some(machine) = self.machines.iter_mut().find(|m| m.id == id) else {
            return;
        };
        let first = machine.snapshot.is_none();
        for agent in &snapshot.agents {
            let thread = threads::thread_id(id, &agent.pane_id);
            if first || self.activity.knows(&thread) {
                self.activity.observe(&thread, agent.agent_status, now);
            } else {
                self.activity.appear(&thread, agent.agent_status, now);
            }
        }
        machine.connection = Connection::Live;
        machine.snapshot = Some(snapshot);
        machine.checkouts = checkouts;
        let live: HashSet<ThreadId> = self
            .machines
            .iter()
            .filter_map(|m| m.snapshot.as_ref().map(|s| (m.id.as_str(), s)))
            .flat_map(|(machine, s)| s.agents.iter().map(move |a| threads::thread_id(machine, &a.pane_id)))
            .collect();
        self.activity.retain(&live);
        self.rebuild();

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
        if self.confirm_archive.as_ref().is_some_and(|c| !live.contains(c)) {
            self.confirm_archive = None;
        }
        self.fix_cursor();
        if self.open.is_some() {
            return;
        }
        // A resume target only matters for the snapshot that brings its
        // machine back; after that it is forgotten either way.
        let resume = self.resume.take_if(|r| r.starts_with(&format!("{id}/")));
        if let Some(resume) = resume.filter(|r| live.contains(r)) {
            self.cursor = Some(resume.clone());
            self.open_thread(&resume, now, effects);
        } else if !self.auto_opened
            && let Some(cursor) = self.cursor.clone()
        {
            self.auto_opened = true;
            self.open_thread(&cursor, now, effects);
        }
    }

    fn on_status(&mut self, id: &str, change: AgentStatusChange, now: SystemTime) {
        let Some(snapshot) = self.machine_mut(id).and_then(|m| m.snapshot.as_mut()) else {
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
        let thread = threads::thread_id(id, &change.pane_id);
        self.activity.observe(&thread, change.agent_status, now);
        // Finishing while the user watches it is not news.
        let watching = self.focus == Focus::Terminal && self.open.as_ref().is_some_and(|o| o.id == thread);
        if watching && change.agent_status == AgentStatus::Done {
            self.activity.mark_seen(&thread);
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let sources: Vec<Source> = self
            .machines
            .iter()
            .filter_map(|m| {
                let snapshot = m.snapshot.as_ref()?;
                Some(Source {
                    machine_id: &m.id,
                    machine_label: (!m.is_local()).then_some(m.label.as_str()),
                    snapshot,
                    checkouts: &m.checkouts,
                })
            })
            .collect();
        self.threads = threads::build(&sources, &self.activity);
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
        let (machine, pane_id) = (thread.machine_id.clone(), thread.pane_id.clone());
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
            machine: machine.clone(),
            pane_id: pane_id.clone(),
            generation,
            screen: Screen::new(cols, rows),
            stream: StreamState::Attaching,
        });
        effects.push(Effect::Attach { generation, machine, pane_id, cols, rows });
        self.mark_seen(id, now);
    }

    fn mark_seen(&mut self, id: &str, _now: SystemTime) {
        self.activity.mark_seen(id);
        self.rebuild();
    }

    /// Asks for the local server, from the full screen or the list.
    pub(crate) fn start_local_server(&mut self, now: SystemTime, effects: &mut Vec<Effect>) {
        if let Some(local) = self.machine_mut(threads::LOCAL) {
            local.connection = Connection::Starting(now);
        }
        self.notice = None;
        effects.push(Effect::StartServer);
    }
}

#[cfg(test)]
mod tests;
