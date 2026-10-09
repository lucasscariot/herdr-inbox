//! Runs the inbox: owns the terminal, the link to Herdr and the agent's live
//! session, and performs the app's effects.

use std::collections::HashMap;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};

use crate::app::{App, Effect, Input};
use crate::cli::Options;
use crate::config::Config;
use crate::discovery::{self, Inventory};
use crate::herdr::socket::{self, Endpoint, SocketEnv};
use crate::herdr::terminal::{HerdrCommand, Session};
use crate::herdr::transport::{Runner, Transport};
use crate::launch::{self, Plan, Record, Stage};
use crate::link::{Link, Request};
use crate::machines::{self, Remote};
use crate::speech::backends;
use crate::speech::meter::Meter;
use crate::speech::recorder::{self, Recording};
use crate::speech::whisper;
use crate::state::State;
use crate::theme;
use crate::threads;

/// How often the screen refreshes on its own, for ages and notices.
const TICK: Duration = Duration::from_millis(500);
/// The shortest time between two draws while events stream in.
const FRAME: Duration = Duration::from_millis(16);
/// How often the orbit moves while it is on screen.
const ANIMATION: Duration = Duration::from_millis(100);

pub fn run(options: Options) -> anyhow::Result<()> {
    let env = SocketEnv::from_process();
    let endpoint = socket::resolve(options.session.as_deref(), &env)?;
    let palette = theme::load(&socket::config_dir(&env)?.join("config.toml"));
    let herdr = herdr_command(&options, &endpoint);
    let paths = match &options.config {
        // An explicit file is the only one read; no legacy fallback.
        Some(path) => Some(crate::config::Paths { config: path.clone(), legacy: path.with_extension("none") }),
        None => crate::config::Paths::from_env(&env),
    };
    let (config, config_error) = match paths.map(|paths| crate::config::load(&paths)) {
        Some(Ok(config)) => (config, None),
        Some(Err(err)) => (Config::default(), Some(err.to_string())),
        None => (Config::default(), None),
    };
    let state = State::new(State::default_dir().unwrap_or_else(|| std::env::temp_dir().join("herdr-inbox")));

    let mut terminal = setup_terminal()?;
    let setup = Setup { endpoint: &endpoint, herdr: &herdr, palette: &palette, config, config_error, state };
    let result = event_loop(&mut terminal, setup);
    restore_terminal();
    result
}

fn herdr_command(options: &Options, endpoint: &Endpoint) -> HerdrCommand {
    let mut command = HerdrCommand::new(&options.herdr);
    if let Some(session) = &endpoint.session {
        command.prefix = vec!["--session".into(), session.into()];
    }
    // Pin every subprocess to the server this inbox shows.
    command.env = vec![("HERDR_SOCKET_PATH".into(), endpoint.api_socket.clone().into_os_string())];
    command
}

fn setup_terminal() -> anyhow::Result<DefaultTerminal> {
    install_panic_hook();
    let terminal = ratatui::try_init().context("cannot take over this terminal")?;
    let mut out = io::stdout();
    execute!(out, EnableMouseCapture, EnableBracketedPaste)?;
    // Tells Shift+Enter from Enter and Tab from Ctrl+I, where the terminal can.
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    Ok(terminal)
}

fn restore_terminal() {
    let mut out = io::stdout();
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    let _ = out.flush();
}

/// Restores the terminal before printing a panic, except for panics the
/// terminal emulator catches and recovers from.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if crate::screen::recovering() {
            return;
        }
        restore_terminal();
        default(info);
    }));
}

/// Every machine's transport and link, by machine id.
struct Fleet {
    machines: HashMap<String, (Transport, Link)>,
}

impl Fleet {
    fn start(local: Transport, remotes: Vec<Remote>, tx: &Sender<Input>) -> Self {
        let mut machines = HashMap::new();
        let mut all = vec![(threads::LOCAL.to_string(), local)];
        all.extend(remotes.into_iter().map(|r| (r.info.id, Transport::Ssh(r.ssh))));
        for (id, transport) in all {
            let deliver = tx.clone();
            let link = Link::spawn(id.clone(), transport.clone(), move |input| {
                let _ = deliver.send(input);
            });
            machines.insert(id, (transport, link));
        }
        Self { machines }
    }

    fn get(&self, id: &str) -> Option<&(Transport, Link)> {
        self.machines.get(id)
    }
}

/// Everything the event loop starts from.
struct Setup<'a> {
    endpoint: &'a Endpoint,
    herdr: &'a HerdrCommand,
    palette: &'a theme::Palette,
    config: Config,
    config_error: Option<String>,
    state: State,
}

fn event_loop(terminal: &mut DefaultTerminal, setup: Setup) -> anyhow::Result<()> {
    let Setup { endpoint, herdr, palette, config, config_error, state } = setup;
    let size = terminal.size()?;
    let local = Transport::Local { socket: endpoint.api_socket.clone(), herdr: herdr.clone() };
    let mut remotes = machines::load(&local.runner());
    // A different ssh program, for wrappers and for tests.
    if let Some(ssh) = std::env::var_os("HERDR_INBOX_SSH").filter(|v| !v.is_empty()) {
        for remote in &mut remotes {
            remote.ssh.ssh = ssh.clone().into();
        }
    }
    if !remotes.is_empty() {
        let dir = crate::herdr::ssh::default_control_dir();
        // Without a private control directory SSH still works, just slower.
        let _ = crate::herdr::ssh::prepare_control_dir(&dir);
    }
    if let Some(legacy) = State::legacy_dir() {
        state.migrate_from(&legacy);
    }
    let mut app = App::new(size.width, size.height, remotes.iter().map(|r| r.info.clone()).collect())
        .with_setup(config.clone(), state.preferences())
        .with_memory(state.presets(), state.history());
    if let Some(error) = config_error {
        app.notify(format!("Config ignored: {error}"), crate::app::NoticeKind::Error, SystemTime::now());
    }
    let (tx, rx) = mpsc::channel::<Input>();
    spawn_input_reader(tx.clone());
    let labels: HashMap<String, (String, bool)> =
        app.machines.iter().map(|m| (m.id.clone(), (m.label.clone(), m.is_local()))).collect();
    let fleet = Fleet::start(local, remotes, &tx);
    let credentials: backends::Credentials = state.read("credentials.json");
    let voice = Voice::new(config.speech.clone(), credentials.clone());
    app.update(Input::Speech(voice.status()), SystemTime::now());
    let mut app = app.with_credentials(credentials);
    let work = Work { state, config, voice, labels, tx: tx.clone() };
    work.restore(&mut app);
    let mut session: Option<(u64, Session)> = None;
    let mut last_draw = Instant::now() - FRAME;
    let mut dirty = true;
    loop {
        if dirty && last_draw.elapsed() >= FRAME {
            let now = SystemTime::now();
            terminal.draw(|frame| crate::ui::draw(frame, &app, palette, now))?;
            last_draw = Instant::now();
            dirty = false;
        }
        let timeout = match (dirty, app.animating()) {
            (true, _) => FRAME.saturating_sub(last_draw.elapsed()),
            (false, true) => ANIMATION,
            (false, false) => TICK,
        };
        let input = match rx.recv_timeout(timeout) {
            Ok(input) => input,
            Err(RecvTimeoutError::Timeout) => {
                if !dirty {
                    app.update(Input::Tick, SystemTime::now());
                    dirty = true;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        // Handle everything already queued before drawing again.
        for input in std::iter::once(input).chain(drain(&rx)) {
            let effects = app.update(input, SystemTime::now());
            for effect in effects {
                if !perform(effect, &mut session, herdr, &fleet, &work) {
                    if let Some((_, mut session)) = session.take() {
                        session.release();
                    }
                    work.voice.cancel();
                    return Ok(());
                }
            }
        }
        dirty = true;
    }
}

fn drain(rx: &Receiver<Input>) -> Vec<Input> {
    rx.try_iter().take(512).collect()
}

/// Performs one effect. Returns false to quit.
fn perform(
    effect: Effect,
    session: &mut Option<(u64, Session)>,
    herdr: &HerdrCommand,
    fleet: &Fleet,
    work: &Work,
) -> bool {
    let tx = &work.tx;
    match effect {
        Effect::Quit => return false,
        Effect::Detach => {
            if let Some((_, mut old)) = session.take() {
                old.release();
            }
        }
        Effect::Attach { generation, machine, pane_id, cols, rows } => {
            if let Some((_, mut old)) = session.take() {
                old.release();
            }
            let closed = |reason: String| Input::Terminal {
                generation,
                message: crate::herdr::terminal::Message::Closed { reason: Some(reason) },
            };
            let Some((transport, _)) = fleet.get(&machine) else {
                let _ = tx.send(closed(format!("unknown machine {machine}")));
                return true;
            };
            let runner = transport.runner();
            let deliver = tx.clone();
            let spawned = Session::spawn(&runner, &pane_id, cols, rows, move |message| {
                let _ = deliver.send(Input::Terminal { generation, message });
            });
            match spawned {
                Ok(new) => *session = Some((generation, new)),
                Err(err) => {
                    let _ = tx.send(closed(format!("cannot run {}: {err}", runner.program())));
                }
            }
        }
        Effect::Send { generation, control } => {
            if let Some((current, live)) = session.as_mut()
                && *current == generation
            {
                // A failed write means the stream is ending; its close
                // message explains why.
                let _ = live.send(&control);
            }
        }
        Effect::Archive { machine, workspace_id, title, .. } => {
            let Some((transport, link)) = fleet.get(&machine) else {
                return true;
            };
            let transport = transport.clone();
            let tx = tx.clone();
            thread::spawn(move || {
                let result = transport
                    .call("workspace.close", serde_json::json!({"workspace_id": workspace_id, "close_group": false}))
                    .map(|_| ())
                    .map_err(|err| err.to_string());
                let _ = tx.send(Input::Archived { title, result });
            });
            link.request(Request::Refresh);
        }
        Effect::StartServer => {
            start_server(herdr);
            if let Some((_, link)) = fleet.get(threads::LOCAL) {
                link.request(Request::Refresh);
            }
        }
        Effect::Discover { machine, include_models } => {
            if let Some((transport, _)) = fleet.get(&machine) {
                work.discover(machine, transport.clone(), include_models);
            }
        }
        Effect::Launch { machine, plan } => {
            if let Some((transport, link)) = fleet.get(&machine) {
                work.launch(transport.runner(), *plan, link.refresher());
            }
        }
        Effect::Resume { machine, record } => {
            if let Some((transport, _)) = fleet.get(&machine) {
                work.resume(transport.runner(), *record);
            }
        }
        Effect::Remember(remembered) => {
            // Losing a remembered choice only costs a default next time.
            let _ = work.state.remember(&remembered);
        }
        Effect::SavePresets(presets) => {
            if let Err(err) = work.state.save_presets(&presets) {
                let _ = tx.send(Input::Error(format!("Could not save presets: {err}")));
            }
        }
        Effect::SaveHistory(task) => {
            let _ = work.state.push_history(&task);
        }
        Effect::Prompt { machine, pane_id, title, text } => {
            if let Some((transport, _)) = fleet.get(&machine) {
                let runner = transport.runner();
                let tx = tx.clone();
                thread::spawn(move || {
                    let result = launch::prompt(&runner, &pane_id, &text);
                    let _ = tx.send(Input::Prompted { title, text, result });
                });
            }
        }
        Effect::Dismiss { record } => {
            let _ = work.state.remove(&format!("launches/{record}.json"));
        }
        Effect::StartDictation => work.voice.start(tx),
        Effect::StopDictation => work.voice.stop(tx),
        Effect::CancelDictation => work.voice.cancel(),
        Effect::TypeText { machine, pane_id, title, text } => {
            if let Some((transport, _)) = fleet.get(&machine) {
                let runner = transport.runner();
                let tx = tx.clone();
                thread::spawn(move || {
                    let result =
                        launch::herdr(&runner, &["pane", "send-text", &pane_id, &text], Duration::from_secs(15))
                            .map(|_| ())
                            .map_err(|e| e.message);
                    let _ = tx.send(Input::Typed { title, result });
                });
            }
        }
        Effect::OpenUrl(url) => open_url(&url),
        Effect::VerifyKey { service, key } => {
            let tx = tx.clone();
            thread::spawn(move || {
                let result = backends::verify_key(service, &key, "curl");
                let _ = tx.send(Input::KeyVerified { service, key, result });
            });
        }
        Effect::InstallWhisper => {
            let tx = tx.clone();
            thread::spawn(move || {
                let progress = |text: &str| {
                    let _ = tx.send(Input::Whisper { result: Ok(None), progress: Some(text.to_string()) });
                };
                let tools = whisper::Tools::system();
                let result = whisper::install(
                    &tools,
                    &whisper::home(),
                    whisper::MODEL,
                    &whisper::model_url(whisper::MODEL),
                    &progress,
                );
                let _ = tx.send(Input::Whisper { result: result.map(Some), progress: None });
            });
        }
        Effect::SaveCredentials(credentials) => {
            if let Err(err) = work.state.write("credentials.json", &credentials) {
                let _ = tx.send(Input::Error(format!("Could not save dictation settings: {err}")));
            }
            work.voice.set_credentials(credentials);
            let _ = tx.send(Input::Speech(work.voice.status()));
        }
    }
    true
}

/// Opens a page in the user's browser, detached.
fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut command = Command::new(opener);
    command.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _ = command.spawn();
}

/// The microphone: at most one recording, its meter, and how to transcribe.
struct Voice {
    speech: backends::SpeechConfig,
    credentials: std::sync::Mutex<backends::Credentials>,
    recording: std::sync::Mutex<Option<(Recording, std::sync::Arc<std::sync::atomic::AtomicBool>)>>,
}

impl Voice {
    fn new(speech: backends::SpeechConfig, credentials: backends::Credentials) -> Self {
        Self { speech, credentials: std::sync::Mutex::new(credentials), recording: std::sync::Mutex::new(None) }
    }

    fn settings(&self) -> backends::Settings {
        let credentials = self.credentials.lock().unwrap_or_else(|e| e.into_inner());
        backends::Settings::merge(&self.speech, &credentials)
    }

    fn set_credentials(&self, credentials: backends::Credentials) {
        *self.credentials.lock().unwrap_or_else(|e| e.into_inner()) = credentials;
    }

    fn status(&self) -> crate::app::SpeechStatus {
        let environment = backends::Environment::system();
        let available = backends::available(&self.settings(), &environment);
        let tools = available
            .iter()
            .filter(|b| matches!(b, backends::Backend::Tool { .. } | backends::Backend::Command(_)))
            .map(backends::Backend::describe)
            .collect();
        crate::app::SpeechStatus { ready: available.first().map(backends::Backend::describe), tools }
    }

    fn start(&self, tx: &Sender<Input>) {
        self.cancel();
        let path = Recording::new_path();
        let Some(argv) = recorder::recorder_argv(&path, &recorder::installed) else {
            let message =
                "No microphone recorder found. Install pipewire (pw-record), alsa-utils (arecord), sox, or ffmpeg.";
            let _ = tx.send(Input::DictationStarted(Err(message.into())));
            return;
        };
        match Recording::start(&argv, path.clone()) {
            Ok(recording) => {
                let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let flag = std::sync::Arc::clone(&running);
                let meter_tx = tx.clone();
                thread::spawn(move || {
                    let mut meter = Meter::new(path);
                    while flag.load(std::sync::atomic::Ordering::Relaxed) {
                        let levels = meter.update().to_vec();
                        if meter_tx.send(Input::Levels { levels, quiet: meter.quiet() }).is_err() {
                            return;
                        }
                        thread::sleep(Duration::from_millis(66));
                    }
                });
                *self.recording.lock().unwrap_or_else(|e| e.into_inner()) = Some((recording, running));
                let _ = tx.send(Input::DictationStarted(Ok(())));
            }
            Err(error) => {
                let _ = tx.send(Input::DictationStarted(Err(error)));
            }
        }
    }

    fn stop(&self, tx: &Sender<Input>) {
        let Some((recording, running)) = self.recording.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            let _ = tx.send(Input::Transcribed(Err("Nothing was recording.".into())));
            return;
        };
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        let settings = self.settings();
        let tx = tx.clone();
        thread::spawn(move || {
            let result = recording.stop().and_then(|path| {
                let environment = backends::Environment::system();
                let text = backends::transcribe(&path, &settings, &environment, "curl");
                let _ = std::fs::remove_file(&path);
                text
            });
            let _ = tx.send(Input::Transcribed(result));
        });
    }

    fn cancel(&self) {
        if let Some((recording, running)) = self.recording.lock().unwrap_or_else(|e| e.into_inner()).take() {
            running.store(false, std::sync::atomic::Ordering::Relaxed);
            recording.cancel();
        }
    }
}

/// Background work: discovery, launches, dictation, and the state they write.
struct Work {
    state: State,
    config: Config,
    voice: Voice,
    /// Machine id → (label, is local), for per-machine settings.
    labels: HashMap<String, (String, bool)>,
    tx: Sender<Input>,
}

impl Work {
    /// Delivers what an earlier run left: cached inventories, and launches
    /// still waiting at a startup dialog.
    fn restore(&self, app: &mut App) {
        let now = SystemTime::now();
        for machine in self.labels.keys() {
            let cached: Inventory = self.state.read(&inventory_file(machine));
            if !cached.projects.is_empty() || !cached.harnesses.is_empty() {
                app.update(Input::Inventory { machine: machine.clone(), result: Ok(cached) }, now);
            }
        }
        let cutoff = discovery::seconds(now).saturating_sub(JOURNAL_DAYS * 86_400);
        let mut records = Vec::new();
        for id in self.state.list("launches") {
            let file = format!("launches/{id}.json");
            let Ok(record) = serde_json::from_value::<Record>(self.state.read(&file)) else {
                continue;
            };
            // Delivered launches are kept for re-sending, not forever. Failed
            // and waiting ones stay until the user deals with them.
            if record.stage == Stage::Submitted && record.created_at < cutoff {
                let _ = self.state.remove(&file);
                continue;
            }
            records.push(record);
        }
        if !records.is_empty() {
            let _ = self.tx.send(Input::Journals(records));
        }
    }

    fn discover(&self, machine: String, transport: Transport, include_models: bool) {
        let (label, local) = self.labels.get(&machine).cloned().unwrap_or_default();
        let effective = self.config.for_machine(&machine, &label, local);
        let state = self.state.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let file = inventory_file(&machine);
            let cached: Inventory = state.read(&file);
            let settings = discovery::settings(&effective, include_models);
            let result = transport
                .probe(discovery::PROBE, &settings, DISCOVERY_TIMEOUT)
                .map_err(|err| err.to_string())
                .and_then(|output| {
                    discovery::finish(&output, Some(&cached), include_models, &effective.models, SystemTime::now())
                });
            if let Ok(inventory) = &result {
                let _ = state.write(&file, inventory);
            }
            let _ = tx.send(Input::Inventory { machine, result });
        });
    }

    fn launch(&self, runner: Runner, plan: Plan, refresh: impl Fn() + Send + 'static) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let id = plan.record.id.clone();
            let journal = |record: &Record| {
                let _ = state.write(&format!("launches/{}.json", record.id), record);
            };
            let progress = |text: &str| {
                let _ = tx.send(Input::LaunchProgress { id: id.clone(), text: text.to_string() });
            };
            let result = launch::execute(&runner, plan, &journal, &progress);
            refresh();
            let _ = tx.send(Input::LaunchFinished { id, result });
        });
    }

    fn resume(&self, runner: Runner, record: Record) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let journal = |record: &Record| {
                let _ = state.write(&format!("launches/{}.json", record.id), record);
            };
            let result = launch::resume(&runner, record, &journal);
            let _ = tx.send(Input::Resumed { result });
        });
    }
}

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(45);
/// How long delivered launches are kept for re-sending.
const JOURNAL_DAYS: u64 = 14;

fn inventory_file(machine: &str) -> String {
    let safe: String = machine.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("inventory/{safe}.json")
}

/// Starts `herdr server` detached, exactly as `herdr` itself would.
fn start_server(herdr: &HerdrCommand) {
    let mut command: Command = herdr.command();
    command.arg("server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so closing the inbox never stops the server.
        command.process_group(0);
    }
    let _ = command.spawn();
}

fn spawn_input_reader(tx: Sender<Input>) {
    thread::spawn(move || {
        loop {
            let input = match event::read() {
                Ok(Event::Key(key)) => Input::Key(key),
                Ok(Event::Mouse(mouse)) => Input::Mouse(mouse),
                Ok(Event::Paste(text)) => Input::Paste(text),
                Ok(Event::Resize(width, height)) => Input::Resize { width, height },
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.send(input).is_err() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn subprocesses_are_pinned_to_the_shown_server() {
        let options = Options { session: None, herdr: "/bin/herdr".into(), config: None };
        let endpoint = Endpoint { api_socket: PathBuf::from("/h/.config/herdr/herdr.sock"), session: None };
        let command = herdr_command(&options, &endpoint);
        assert_eq!(command.program, PathBuf::from("/bin/herdr"));
        assert!(command.prefix.is_empty());
        assert_eq!(command.env, vec![("HERDR_SOCKET_PATH".into(), "/h/.config/herdr/herdr.sock".into())]);
    }

    #[test]
    fn a_named_session_is_passed_explicitly() {
        let options = Options { session: Some("night".into()), herdr: "herdr".into(), config: None };
        let endpoint = Endpoint {
            api_socket: PathBuf::from("/h/.config/herdr/sessions/night/herdr.sock"),
            session: Some("night".into()),
        };
        let command = herdr_command(&options, &endpoint);
        assert_eq!(command.prefix, vec![std::ffi::OsString::from("--session"), "night".into()]);
    }
}
