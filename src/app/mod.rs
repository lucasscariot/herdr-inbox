//! The inbox's state machine. `App::update` turns one input into state changes
//! and a list of effects for the runtime to perform; nothing in here does I/O,
//! so every behaviour is testable without a server or a terminal.

mod composer;
mod input;
mod layout;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};

use crate::config::Config;
use crate::discovery::Inventory;
use crate::git::Checkout;
use crate::herdr::terminal::{Control, Message};
use crate::herdr::types::{AgentStatus, AgentStatusChange, SessionSnapshot};
use crate::launch::{self, Failure, Outcome, Plan, Record};
use crate::screen::Screen;
use crate::state::{Preferences, Remembered};
use crate::threads::{self, Activity, Source, Thread, ThreadId};

pub use composer::{Choice, Composer, Field, Pick, Picker, WorkspaceSel};
pub use layout::{Layout, Row, RowKind};

const NOTICE_TTL: Duration = Duration::from_secs(4);
/// How long a server the user asked for may take to answer.
const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Terminal,
    Composer,
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

/// How the launches the user sent are doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchState {
    Running(String),
    Sent {
        unverified: bool,
    },
    /// The agent waits at a startup dialog; the task follows.
    Waiting,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchView {
    pub id: String,
    pub title: String,
    pub project: String,
    pub harness: String,
    pub machine_label: String,
    pub state: LaunchState,
}

/// Launches kept on screen; older ones scroll away.
const LAUNCH_HISTORY: usize = 8;

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
    /// Read a machine's projects and agent CLIs; models too when asked.
    Discover {
        machine: String,
        include_models: bool,
    },
    Launch {
        machine: String,
        plan: Box<Plan>,
    },
    /// Send the task of a launch whose startup dialog was answered.
    Resume {
        machine: String,
        record: Box<Record>,
    },
    Remember(Remembered),
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Snapshot {
        machine: String,
        snapshot: SessionSnapshot,
        checkouts: HashMap<String, Checkout>,
    },
    Status {
        machine: String,
        change: AgentStatusChange,
    },
    Connection {
        machine: String,
        connection: Connection,
    },
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Resize {
        width: u16,
        height: u16,
    },
    Terminal {
        generation: u64,
        message: Message,
    },
    Archived {
        title: String,
        result: Result<(), String>,
    },
    Inventory {
        machine: String,
        result: Result<Inventory, String>,
    },
    LaunchProgress {
        id: String,
        text: String,
    },
    LaunchFinished {
        id: String,
        result: Result<Outcome, Failure>,
    },
    Resumed {
        result: Result<Outcome, Failure>,
    },
    /// Launches from an earlier run that still wait at a startup dialog.
    Journals(Vec<Record>),
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
    /// Threads the user asked to archive, so their disappearance is expected.
    archiving: HashSet<ThreadId>,
    pub config: Config,
    pub preferences: Preferences,
    pub inventories: HashMap<String, Inventory>,
    /// Machines being discovered right now.
    pub discovering: HashSet<String>,
    /// Why a machine's discovery failed, until it succeeds.
    pub discovery_errors: HashMap<String, String>,
    /// The composer's draft, kept while it is closed.
    pub composer: Composer,
    pub launches: Vec<LaunchView>,
    /// Launches waiting at a startup dialog, by thread.
    pub waiting: HashMap<ThreadId, Record>,
    launch_counter: u32,
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
            archiving: HashSet::new(),
            config: Config::default(),
            preferences: Preferences::default(),
            inventories: HashMap::new(),
            discovering: HashSet::new(),
            discovery_errors: HashMap::new(),
            composer: Composer::default(),
            launches: Vec::new(),
            waiting: HashMap::new(),
            launch_counter: 0,
        }
    }

    /// Settings and remembered choices, read at startup.
    pub fn with_setup(mut self, config: Config, preferences: Preferences) -> Self {
        self.config = config;
        self.preferences = preferences;
        self
    }

    /// Runs `f` on the composer with the context it reads, borrowing the
    /// two disjointly.
    pub(crate) fn with_composer<R>(&mut self, f: impl FnOnce(&mut Composer, &composer::Context) -> R) -> R {
        let ctx = composer::Context {
            machines: &self.machines,
            inventories: &self.inventories,
            preferences: &self.preferences,
            config: &self.config,
        };
        f(&mut self.composer, &ctx)
    }

    pub fn composer_context(&self) -> composer::Context<'_> {
        composer::Context {
            machines: &self.machines,
            inventories: &self.inventories,
            preferences: &self.preferences,
            config: &self.config,
        }
    }

    pub fn update(&mut self, input: Input, now: SystemTime) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Snapshot { machine, snapshot, checkouts } => {
                self.on_snapshot(&machine, snapshot, checkouts, now, &mut effects);
                self.after_threads_changed(&mut effects);
            }
            Input::Status { machine, change } => {
                self.on_status(&machine, change, now);
                self.after_threads_changed(&mut effects);
            }
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
            Input::Inventory { machine, result } => self.on_inventory(&machine, result),
            Input::LaunchProgress { id, text } => self.set_launch(&id, LaunchState::Running(text)),
            Input::LaunchFinished { id, result } => self.on_launch_finished(&id, result, now, &mut effects),
            Input::Resumed { result } => self.on_resumed(result, now),
            Input::Journals(records) => {
                for record in records {
                    if let Some(pane) = &record.pane_id {
                        self.waiting.insert(threads::thread_id(&record.machine_id, pane), record);
                    }
                }
                self.resume_ready(&mut effects);
            }
            Input::Tick => {
                if self.notice.as_ref().is_some_and(|n| n.until <= now) {
                    self.notice = None;
                }
            }
        }
        self.layout.update(&self.threads, self.cursor.as_deref());
        effects
    }

    /// Opens the composer, discovering every reachable machine again.
    pub(crate) fn open_composer(&mut self, now: SystemTime, effects: &mut Vec<Effect>) {
        self.focus = Focus::Composer;
        self.confirm_archive = None;
        self.discover(false, now, effects);
        self.with_composer(|composer, ctx| composer.settle(ctx));
    }

    /// Asks every live machine for its projects; models only when stale or
    /// when `force_models`.
    pub(crate) fn discover(&mut self, force_models: bool, now: SystemTime, effects: &mut Vec<Effect>) {
        for machine in &self.machines {
            if !machine.connection.is_live() || self.discovering.contains(&machine.id) {
                continue;
            }
            let fresh = self.inventories.get(&machine.id).is_some_and(|i| i.models_fresh(now));
            self.discovering.insert(machine.id.clone());
            effects.push(Effect::Discover { machine: machine.id.clone(), include_models: force_models || !fresh });
        }
    }

    fn on_inventory(&mut self, machine: &str, result: Result<Inventory, String>) {
        self.discovering.remove(machine);
        match result {
            Ok(inventory) => {
                self.discovery_errors.remove(machine);
                self.inventories.insert(machine.to_string(), inventory);
            }
            Err(error) => {
                self.discovery_errors.insert(machine.to_string(), error);
            }
        }
        self.with_composer(|composer, ctx| composer.settle(ctx));
    }

    /// Validates the composer and starts a launch. `keep` leaves the task in
    /// place to send it again elsewhere.
    pub(crate) fn send(&mut self, keep: bool, now: SystemTime, effects: &mut Vec<Effect>) {
        let ctx = self.composer_context();
        let request = match self.composer.request(&ctx) {
            Ok(request) => request,
            Err(error) => {
                self.composer.error = Some(error);
                return;
            }
        };
        let machine = self.machines.iter().find(|m| m.id == request.machine_id);
        let settings = machine
            .map(|m| self.config.for_machine(&m.id, &m.label, m.is_local()))
            .unwrap_or_else(|| self.config.for_machine(&request.machine_id, &request.machine_label, false));
        let catalog = self.inventories.get(&request.machine_id).and_then(|i| i.models.get(&request.harness));
        self.launch_counter += 1;
        let id = format!("{:08x}{:08x}", crate::discovery::seconds(now) as u32, self.launch_counter);
        let plan = match launch::plan(&request, &settings, catalog, id, now) {
            Ok(plan) => plan,
            Err(error) => {
                self.composer.error = Some(error);
                return;
            }
        };
        let record = &plan.record;
        self.launches.push(LaunchView {
            id: record.id.clone(),
            title: record.title.clone(),
            project: record.project.clone(),
            harness: crate::threads::harness_label_for(&record.harness),
            machine_label: record.machine_label.clone(),
            state: LaunchState::Running("starting".into()),
        });
        if self.launches.len() > LAUNCH_HISTORY {
            self.launches.remove(0);
        }
        self.composer.error = None;
        if !keep {
            self.composer.task.clear();
        }
        if matches!(self.composer.workspace, WorkspaceSel::Named(_)) {
            // A named branch is used once; the next task gets its own.
            self.composer.workspace = WorkspaceSel::New;
        }
        effects.push(Effect::Launch { machine: request.machine_id, plan: Box::new(plan) });
    }

    fn set_launch(&mut self, id: &str, state: LaunchState) {
        if let Some(view) = self.launches.iter_mut().find(|v| v.id == id) {
            view.state = state;
        }
    }

    fn on_launch_finished(
        &mut self,
        id: &str,
        result: Result<Outcome, Failure>,
        now: SystemTime,
        effects: &mut Vec<Effect>,
    ) {
        match result {
            Ok(outcome) => {
                let record = match &outcome {
                    Outcome::Sent(record) | Outcome::WaitingForStartup(record) => record.clone(),
                };
                let workspace = if record.workspace == "worktree" { "worktree" } else { "checkout" };
                let remembered = Remembered {
                    project: record.project.clone(),
                    machine: record.machine_id.clone(),
                    harness: record.harness.clone(),
                    workspace: workspace.into(),
                    model: record.model.clone(),
                    thinking: record.thinking.clone(),
                };
                self.preferences.remember(&remembered);
                effects.push(Effect::Remember(remembered));
                match outcome {
                    Outcome::Sent(record) => self.set_launch(id, LaunchState::Sent { unverified: record.unverified }),
                    Outcome::WaitingForStartup(record) => {
                        self.set_launch(id, LaunchState::Waiting);
                        if let Some(pane) = &record.pane_id {
                            self.waiting.insert(threads::thread_id(&record.machine_id, pane), record);
                        }
                        self.resume_ready(effects);
                    }
                }
            }
            Err(Failure { record, error }) => {
                let first_line = error.lines().next().unwrap_or("launch failed").to_string();
                self.set_launch(id, LaunchState::Failed(first_line.clone()));
                self.notify(format!("Could not start “{}”: {first_line}", record.title), NoticeKind::Error, now);
            }
        }
    }

    fn on_resumed(&mut self, result: Result<Outcome, Failure>, now: SystemTime) {
        match result {
            Ok(Outcome::Sent(record)) => {
                self.set_launch(&record.id, LaunchState::Sent { unverified: record.unverified })
            }
            // Another dialog: wait for the next idle.
            Ok(Outcome::WaitingForStartup(record)) => {
                self.set_launch(&record.id, LaunchState::Waiting);
                if let Some(pane) = &record.pane_id {
                    self.waiting.insert(threads::thread_id(&record.machine_id, pane), record);
                }
            }
            Err(Failure { record, error }) => {
                let first_line = error.lines().next().unwrap_or("send failed").to_string();
                self.set_launch(&record.id, LaunchState::Failed(first_line.clone()));
                self.notify(format!("Could not send “{}”: {first_line}", record.title), NoticeKind::Error, now);
            }
        }
    }

    /// Sends waiting tasks whose agent is now idle: the user answered the
    /// startup dialog.
    fn resume_ready(&mut self, effects: &mut Vec<Effect>) {
        let ready: Vec<ThreadId> = self
            .waiting
            .keys()
            .filter(|id| self.thread(id).is_some_and(|t| matches!(t.status, AgentStatus::Idle | AgentStatus::Done)))
            .cloned()
            .collect();
        for id in ready {
            if let Some(record) = self.waiting.remove(&id) {
                self.set_launch(&record.id, LaunchState::Running("sending the task".into()));
                effects.push(Effect::Resume { machine: record.machine_id.clone(), record: Box::new(record) });
            }
        }
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

        if let Some(open) = self.open.take_if(|o| !live.contains(&o.id)) {
            effects.push(Effect::Detach);
            if self.focus == Focus::Terminal {
                self.focus = Focus::List;
            }
            // A thread the user archived is expected to go; its own notice
            // says so, and must not be replaced by this one.
            if !self.archiving.remove(&open.id) {
                self.notify("The thread you had open ended", NoticeKind::Info, now);
            }
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

    fn after_threads_changed(&mut self, effects: &mut Vec<Effect>) {
        self.resume_ready(effects);
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
