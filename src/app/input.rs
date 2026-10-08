//! Keyboard, mouse and paste handling.
//!
//! With the terminal focused, every key goes to the agent except Tab, which
//! moves to the thread list. In the list, keys navigate and act on threads and
//! nothing reaches an agent.

use std::time::SystemTime;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::{App, Connection, Effect, Field, Focus, StreamState};
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
        Focus::Composer => composer_key(app, key, now, effects),
    }
}

/// Leaves the composer, back to the agent if one is open.
fn close_composer(app: &mut App) {
    app.composer.picker = None;
    app.focus = if app.open.is_some() { Focus::Terminal } else { Focus::List };
}

fn composer_key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if app.composer.picker.is_some() {
        return picker_key(app, key);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc => return close_composer(app),
        KeyCode::Char('c') if ctrl => {
            if app.composer.task.text().is_empty() {
                close_composer(app);
            } else {
                app.composer.task.clear();
            }
            return;
        }
        KeyCode::Char('s') if ctrl => return app.send(true, now, effects),
        KeyCode::Enter if ctrl => return app.send(true, now, effects),
        KeyCode::Tab => return app.with_composer(|c, ctx| c.next_field(ctx, true)),
        KeyCode::BackTab => return app.with_composer(|c, ctx| c.next_field(ctx, false)),
        KeyCode::F(5) => return app.discover(true, now, effects),
        KeyCode::F(n) => {
            if let Some(field) = Field::from_key(n) {
                app.composer.open_picker(field, "");
            }
            return;
        }
        _ => {}
    }
    if app.composer.field != Field::Task {
        return field_key(app, key);
    }
    let editor = &mut app.composer.task;
    match key.code {
        KeyCode::Enter if shift || alt => editor.insert("\n"),
        KeyCode::Enter => app.send(false, now, effects),
        KeyCode::Char('a') if ctrl => editor.home(),
        KeyCode::Char('e') if ctrl => editor.end(),
        KeyCode::Char('u') if ctrl => editor.clear(),
        KeyCode::Char('w') if ctrl => editor.delete_word(),
        KeyCode::Backspace if alt || ctrl => editor.delete_word(),
        KeyCode::Char(c) if !ctrl => {
            let mut buffer = [0; 4];
            editor.insert(c.encode_utf8(&mut buffer));
        }
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Up => {
            editor.up();
        }
        KeyCode::Down => {
            let moved = editor.down();
            // Past the last line, the fields begin.
            if !moved {
                app.with_composer(|c, ctx| c.next_field(ctx, true));
            }
        }
        _ => {}
    }
}

/// Keys on a field row: move between fields, or open its picker.
fn field_key(app: &mut App, key: KeyEvent) {
    let field = app.composer.field;
    match key.code {
        KeyCode::Up => app.with_composer(|c, ctx| c.next_field(ctx, false)),
        KeyCode::Down => app.with_composer(|c, ctx| c.next_field(ctx, true)),
        KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => app.composer.open_picker(field, ""),
        // Typing on a field starts filtering its choices.
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.open_picker(field, &c.to_string())
        }
        _ => {}
    }
}

fn picker_key(app: &mut App, key: KeyEvent) {
    let Some(picker) = app.composer.picker.clone() else {
        return;
    };
    let choices = app.composer.choices(&app.composer_context(), picker.field, &picker.query);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut picker = picker;
    match key.code {
        KeyCode::Esc => {
            app.composer.close_picker();
            return;
        }
        KeyCode::Enter | KeyCode::Tab => {
            if let Some(choice) = choices.get(picker.selected).filter(|c| c.enabled) {
                let pick = choice.pick.clone();
                app.composer.close_picker();
                app.with_composer(|c, ctx| c.apply(ctx, pick));
            }
            return;
        }
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Char('p') if ctrl => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => picker.selected += 1,
        KeyCode::Char('n') if ctrl => picker.selected += 1,
        KeyCode::Backspace => {
            picker.query.pop();
            picker.selected = 0;
        }
        KeyCode::Char(c) if !ctrl => {
            picker.query.push(c);
            picker.selected = 0;
        }
        _ => return,
    }
    // Re-rank for the new query and keep the selection on the list.
    let count = app.composer.choices(&app.composer_context(), picker.field, &picker.query).len();
    picker.selected = picker.selected.min(count.saturating_sub(1));
    app.composer.picker = Some(picker);
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
        KeyCode::Char('n') => app.open_composer(now, effects),
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
    if app.focus == Focus::Composer {
        match &mut app.composer.picker {
            Some(picker) => picker.query.push_str(text.lines().next().unwrap_or("")),
            None if app.composer.field == Field::Task => app.composer.task.insert(text),
            None => {}
        }
        return;
    }
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
    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && app.layout.on_new_button(x, y)
        && !app.needs_server_screen()
    {
        return app.open_composer(now, effects);
    }
    if app.focus == Focus::Composer && app.layout.in_terminal(x, y) {
        return;
    }
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
