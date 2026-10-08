//! Runs the inbox: owns the terminal, the link to Herdr and the agent's live
//! session, and performs the app's effects.

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
use crate::herdr::Api;
use crate::herdr::socket::{self, Endpoint, SocketEnv};
use crate::herdr::terminal::{HerdrCommand, Session};
use crate::link::{Link, Request};
use crate::theme;

/// How often the screen refreshes on its own, for ages and notices.
const TICK: Duration = Duration::from_millis(500);
/// The shortest time between two draws while events stream in.
const FRAME: Duration = Duration::from_millis(16);

pub fn run(options: Options) -> anyhow::Result<()> {
    let env = SocketEnv::from_process();
    let endpoint = socket::resolve(options.session.as_deref(), &env)?;
    let palette = theme::load(&socket::config_dir(&env)?.join("config.toml"));
    let herdr = herdr_command(&options, &endpoint);

    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &endpoint, &herdr, &palette);
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

fn event_loop(
    terminal: &mut DefaultTerminal,
    endpoint: &Endpoint,
    herdr: &HerdrCommand,
    palette: &theme::Palette,
) -> anyhow::Result<()> {
    let size = terminal.size()?;
    let mut app = App::new(size.width, size.height, Box::new(crate::git::checkout));
    let (tx, rx) = mpsc::channel::<Input>();
    spawn_input_reader(tx.clone());
    let link = {
        let tx = tx.clone();
        Link::spawn(endpoint.api_socket.clone(), move |input| {
            let _ = tx.send(input);
        })
    };
    let api = Api::new(endpoint.api_socket.clone());
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
        let timeout = if dirty { FRAME.saturating_sub(last_draw.elapsed()) } else { TICK };
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
                if !perform(effect, &mut session, herdr, &api, &link, &tx) {
                    if let Some((_, mut session)) = session.take() {
                        session.release();
                    }
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
    api: &Api,
    link: &Link,
    tx: &Sender<Input>,
) -> bool {
    match effect {
        Effect::Quit => return false,
        Effect::Detach => {
            if let Some((_, mut old)) = session.take() {
                old.release();
            }
        }
        Effect::Attach { generation, pane_id, cols, rows } => {
            if let Some((_, mut old)) = session.take() {
                old.release();
            }
            let deliver = tx.clone();
            let spawned = Session::spawn(herdr, &pane_id, cols, rows, move |message| {
                let _ = deliver.send(Input::Terminal { generation, message });
            });
            match spawned {
                Ok(new) => *session = Some((generation, new)),
                Err(err) => {
                    let reason = format!("cannot run {}: {err}", herdr.program.display());
                    let _ = tx.send(Input::Terminal {
                        generation,
                        message: crate::herdr::terminal::Message::Closed { reason: Some(reason) },
                    });
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
        Effect::Archive { workspace_id, title, .. } => {
            let api = api.clone();
            let tx = tx.clone();
            thread::spawn(move || {
                let result = api.workspace_close(&workspace_id).map_err(|err| err.to_string());
                let _ = tx.send(Input::Archived { title, result });
            });
            link.request(Request::Refresh);
        }
        Effect::StartServer => {
            start_server(herdr);
            link.request(Request::Refresh);
        }
    }
    true
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
        let options = Options { session: None, herdr: "/bin/herdr".into() };
        let endpoint = Endpoint { api_socket: PathBuf::from("/h/.config/herdr/herdr.sock"), session: None };
        let command = herdr_command(&options, &endpoint);
        assert_eq!(command.program, PathBuf::from("/bin/herdr"));
        assert!(command.prefix.is_empty());
        assert_eq!(command.env, vec![("HERDR_SOCKET_PATH".into(), "/h/.config/herdr/herdr.sock".into())]);
    }

    #[test]
    fn a_named_session_is_passed_explicitly() {
        let options = Options { session: Some("night".into()), herdr: "herdr".into() };
        let endpoint = Endpoint {
            api_socket: PathBuf::from("/h/.config/herdr/sessions/night/herdr.sock"),
            session: Some("night".into()),
        };
        let command = herdr_command(&options, &endpoint);
        assert_eq!(command.prefix, vec![std::ffi::OsString::from("--session"), "night".into()]);
    }
}
