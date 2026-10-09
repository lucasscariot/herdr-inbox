//! Keyboard, mouse and paste handling.
//!
//! With the terminal focused, every key goes to the agent except Tab, which
//! moves to the thread list. In the list, keys navigate and act on threads and
//! nothing reaches an agent.

use std::time::SystemTime;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::{App, Connection, Effect, Focus, StreamState};
use crate::herdr::terminal::{Control, MouseAction, MouseButton as PaneButton, ScrollDirection};
use crate::keys;

/// Lines per mouse wheel notch.
const WHEEL_LINES: u16 = 3;

pub(super) fn key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    if app.needs_server_screen() {
        match app.local().connection {
            Connection::NoServer => no_server_key(app, key, now, effects),
            _ if key.code == KeyCode::Char('q') || is_ctrl_c(key) => effects.push(Effect::Quit),
            _ => {}
        }
        return;
    }
    match app.focus {
        Focus::Terminal => terminal_key(app, key, now, effects),
        Focus::List => list_key(app, key, now, effects),
    }
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn no_server_key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    match key.code {
        KeyCode::Enter => app.start_local_server(now, effects),
        KeyCode::Char('q') | KeyCode::Esc => effects.push(Effect::Quit),
        _ if is_ctrl_c(key) => effects.push(Effect::Quit),
        _ => {}
    }
}

fn terminal_key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if key.code == KeyCode::Tab && key.modifiers.is_empty() {
        app.focus = Focus::List;
        if let Some(open) = &app.open {
            app.cursor = Some(open.id.clone());
        }
        return;
    }
    let Some(open) = &app.open else {
        app.focus = Focus::List;
        return;
    };
    match &open.stream {
        StreamState::Closed { .. } => {
            if key.code == KeyCode::Enter {
                let id = open.id.clone();
                app.open_thread(&id, now, effects);
            }
        }
        StreamState::Attaching | StreamState::Live => {
            if let Some(bytes) = keys::encode(key, open.screen.modes()) {
                effects.push(Effect::Send { generation: open.generation, control: Control::Input(bytes) });
            }
        }
    }
}

fn list_key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if let Some(id) = app.confirm_archive.take() {
        if matches!(key.code, KeyCode::Enter | KeyCode::Char('y'))
            && let Some(thread) = app.thread(&id)
        {
            let effect = Effect::Archive {
                machine: thread.machine_id.clone(),
                thread: thread.id.clone(),
                workspace_id: thread.workspace_id.clone(),
                title: thread.title.clone(),
            };
            app.archiving.insert(id);
            effects.push(effect);
        }
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => effects.push(Effect::Quit),
        KeyCode::Char('q') => effects.push(Effect::Quit),
        KeyCode::Char('j') | KeyCode::Down => move_cursor(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(app, -1),
        KeyCode::Char('g') | KeyCode::Home => select(app, 0),
        KeyCode::Char('G') | KeyCode::End => select(app, app.threads.len().saturating_sub(1)),
        KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') | KeyCode::Right => {
            if let Some(id) = app.cursor.clone() {
                app.open_thread(&id, now, effects);
                app.focus = Focus::Terminal;
            }
        }
        KeyCode::Tab | KeyCode::Esc if app.open.is_some() => {
            app.focus = Focus::Terminal;
        }
        KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
            app.confirm_archive = app.cursor.clone();
        }
        // With saved machines the list stays up without a local server; `s`
        // starts one.
        KeyCode::Char('s') if app.local().connection == Connection::NoServer => app.start_local_server(now, effects),
        _ => {}
    }
}

fn move_cursor(app: &mut App, delta: isize) {
    let Some(current) = app.cursor_index() else {
        if !app.threads.is_empty() {
            select(app, 0);
        }
        return;
    };
    let last = app.threads.len().saturating_sub(1);
    select(app, current.saturating_add_signed(delta).min(last));
}

fn select(app: &mut App, index: usize) {
    if let Some(thread) = app.threads.get(index) {
        app.cursor = Some(thread.id.clone());
    }
}

pub(super) fn paste(app: &mut App, text: &str, effects: &mut Vec<Effect>) {
    if app.focus != Focus::Terminal {
        return;
    }
    if let Some(open) = app.open.as_ref().filter(|o| !matches!(o.stream, StreamState::Closed { .. })) {
        effects.push(Effect::Send {
            generation: open.generation,
            control: Control::Input(keys::paste(text, open.screen.modes())),
        });
    }
}

pub(super) fn mouse(app: &mut App, mouse: MouseEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    let (x, y) = (mouse.column, mouse.row);
    if app.layout.in_list(x, y) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                app.confirm_archive = None;
                if let Some(index) = app.layout.thread_at(x, y) {
                    select(app, index);
                    if let Some(id) = app.cursor.clone() {
                        app.open_thread(&id, now, effects);
                        app.focus = Focus::Terminal;
                    }
                }
            }
            MouseEventKind::ScrollDown => app.layout.scroll(WHEEL_LINES as isize),
            MouseEventKind::ScrollUp => app.layout.scroll(-(WHEEL_LINES as isize)),
            _ => {}
        }
        return;
    }
    if !app.layout.in_terminal(x, y) {
        return;
    }
    let Some(open) = app.open.as_ref().filter(|o| !matches!(o.stream, StreamState::Closed { .. })) else {
        return;
    };
    let generation = open.generation;
    let column = x - app.layout.terminal.x;
    let row = y - app.layout.terminal.y;
    let wants_mouse = open.screen.wants_mouse();
    let control = match mouse.kind {
        MouseEventKind::ScrollUp => Some(Control::Scroll { direction: ScrollDirection::Up, lines: WHEEL_LINES }),
        MouseEventKind::ScrollDown => Some(Control::Scroll { direction: ScrollDirection::Down, lines: WHEEL_LINES }),
        MouseEventKind::Down(button) => {
            app.focus = Focus::Terminal;
            app.confirm_archive = None;
            wants_mouse.then(|| pane_mouse(MouseAction::Down, button, column, row, mouse.modifiers))
        }
        MouseEventKind::Up(button) => {
            wants_mouse.then(|| pane_mouse(MouseAction::Up, button, column, row, mouse.modifiers))
        }
        MouseEventKind::Drag(button) => {
            wants_mouse.then(|| pane_mouse(MouseAction::Drag, button, column, row, mouse.modifiers))
        }
        _ => None,
    };
    if let Some(control) = control {
        effects.push(Effect::Send { generation, control });
    }
}

fn pane_mouse(action: MouseAction, button: MouseButton, column: u16, row: u16, mods: KeyModifiers) -> Control {
    let button = match button {
        MouseButton::Left => PaneButton::Left,
        MouseButton::Right => PaneButton::Right,
        MouseButton::Middle => PaneButton::Middle,
    };
    // Herdr's modifier bits: Shift = 1, Ctrl = 2, Alt = 4.
    let modifiers = u8::from(mods.contains(KeyModifiers::SHIFT))
        | (2 * u8::from(mods.contains(KeyModifiers::CONTROL)))
        | (4 * u8::from(mods.contains(KeyModifiers::ALT)));
    Control::Mouse { action, button, column, row, modifiers }
}
