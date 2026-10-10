//! Keyboard, mouse and paste handling.
//!
//! With the terminal focused, every key goes to the agent except Tab, which
//! moves to the thread list. In the list, keys navigate and act on threads and
//! nothing reaches an agent.

use std::time::SystemTime;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::{App, Connection, Effect, Field, Focus, NoticeKind, Pick, StreamState};
use crate::clipboard::Clip;
use crate::herdr::terminal::{Control, MouseAction, MouseButton as PaneButton, ScrollDirection};
use crate::hold::Step;
use crate::{images, keys};

/// Lines per mouse wheel notch.
const WHEEL_LINES: u16 = 3;

pub(super) fn key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    // A space may be the start of a held bar: it waits until that is clear.
    if is_space(key) && (app.hold.held() || app.space_holds()) {
        match app.hold.space(now) {
            Step::Hold(typed) => {
                type_spaces(app, typed, now, effects);
                app.hold_space(now, effects);
            }
            Step::Type(typed) => type_spaces(app, typed, now, effects),
            Step::Wait | Step::Release => {}
        }
        return;
    }
    let waiting = app.hold.interrupt();
    type_spaces(app, waiting, now, effects);
    dispatch(app, key, now, effects);
}

fn is_space(key: KeyEvent) -> bool {
    key.code == KeyCode::Char(' ') && key.modifiers.is_empty()
}

/// Types spaces that turned out to be typed, not held, where a space would
/// have gone.
pub(super) fn type_spaces(app: &mut App, count: usize, now: SystemTime, effects: &mut Vec<Effect>) {
    for _ in 0..count {
        dispatch(app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), now, effects);
    }
}

/// Types any spaces still waiting, before something else arrives.
pub(super) fn settle_spaces(app: &mut App, now: SystemTime, effects: &mut Vec<Effect>) {
    let waiting = app.hold.interrupt();
    type_spaces(app, waiting, now, effects);
}

fn dispatch(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if app.updates.visible {
        return app.updates.key(key, app.layout.width, app.layout.height, effects);
    }
    if app.menu.is_some() {
        return app.menu_key(key, effects);
    }
    if app.dictation_key(key, effects) {
        return;
    }
    let ctrl_g = key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl_g && (app.focus != Focus::Terminal || app.needs_server_screen()) {
        return app.updates.open(effects);
    }
    let ctrl_t = key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl_t && !app.needs_server_screen() {
        return app.start_dictation(now, effects);
    }
    if key.code == KeyCode::F(10) && matches!(app.focus, Focus::List | Focus::Composer) && !app.needs_server_screen() {
        return app.open_menu();
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
        return picker_key(app, key, effects);
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
        KeyCode::Char('d') if ctrl => {
            let suggested = app.composer.suggested_preset_name(&app.composer_context());
            match suggested {
                Some(name) => {
                    app.composer.open_picker(Field::Preset, &name);
                    if let Some(picker) = app.composer.picker.as_mut() {
                        picker.saving = true;
                    }
                }
                None => app.composer.error = Some("Pick a model first to save a preset.".into()),
            }
            return;
        }
        KeyCode::Char('p') if ctrl && app.composer.field == Field::Task => {
            let history = app.history.clone();
            return app.composer.browse_history(&history, true);
        }
        KeyCode::Char('n') if ctrl && app.composer.field == Field::Task => {
            let history = app.history.clone();
            return app.composer.browse_history(&history, false);
        }
        KeyCode::Enter if ctrl => return app.send(true, now, effects),
        KeyCode::Tab => return app.with_composer(|c, ctx| c.next_field(ctx, true)),
        KeyCode::BackTab => return app.with_composer(|c, ctx| c.next_field(ctx, false)),
        KeyCode::F(5) => return app.discover(true, now, effects),
        KeyCode::F(n) => {
            if let Some(field) = Field::from_key(n)
                && app.composer.fields(&app.composer_context()).contains(&field)
            {
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
        // Ghostty hands over Ctrl+Shift+V (the desktop's paste) as a key when
        // the clipboard holds an image and no text.
        KeyCode::Char('v' | 'V') if ctrl => effects.push(Effect::ReadClipboard),
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

fn picker_key(app: &mut App, key: KeyEvent, effects: &mut Vec<Effect>) {
    let Some(picker) = app.composer.picker.clone() else {
        return;
    };
    let choices = app.composer.choices_with_actions(&app.composer_context(), picker.field, &picker.query);
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
                choose(app, pick, effects);
            }
            return;
        }
        KeyCode::Delete if picker.field == Field::Preset => {
            if let Some(Pick::Preset(name)) = choices.get(picker.selected).map(|c| c.pick.clone()) {
                let next = crate::presets::remove(&app.presets, &name);
                app.change_presets(Ok(next), effects);
            }
        }
        KeyCode::Char('r') if ctrl && picker.field == Field::Preset => {
            if let Some(Pick::Preset(name)) = choices.get(picker.selected).map(|c| c.pick.clone()) {
                picker.query = name.clone();
                picker.renaming = Some(name);
                picker.selected = 0;
            }
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
    app.composer.picker = Some(picker.clone());
    let count = app.composer.choices_with_actions(&app.composer_context(), picker.field, &picker.query).len();
    picker.selected = picker.selected.min(count.saturating_sub(1));
    app.composer.picker = Some(picker);
}

/// Keyboard and mouse selections run the same preset and choice actions.
fn choose(app: &mut App, pick: Pick, effects: &mut Vec<Effect>) {
    app.composer.close_picker();
    match pick {
        Pick::SavePreset(name) => {
            let preset = crate::presets::Preset {
                name,
                harness: app.composer.harness.clone().unwrap_or_default(),
                model: app.composer.model.clone().unwrap_or_default(),
                thinking: app.composer.thinking.clone().unwrap_or_default(),
            };
            let next = crate::presets::save(&app.presets, preset);
            app.change_presets(next, effects);
        }
        Pick::RenamePreset { from, to } => {
            let next = crate::presets::rename(&app.presets, &from, &to);
            app.change_presets(next, effects);
        }
        pick => app.with_composer(|c, ctx| c.apply(ctx, pick)),
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
        app.layout.follow_cursor();
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
    if app.reply.is_some() {
        return reply_key(app, key, now, effects);
    }
    if app.filtering {
        return filter_key(app, key);
    }
    if app.confirm_archive.is_some() && key.kind == KeyEventKind::Repeat {
        // Holding the archive key must not confirm it: only a fresh press does.
        return;
    }
    if let Some(id) = app.confirm_archive.take() {
        // Pressing the archive key again confirms, so archiving is a double tap.
        if matches!(
            key.code,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace
        ) && let Some(thread) = app.thread(&id)
        {
            let failed_launch = thread.id.starts_with(super::LAUNCH_PREFIX);
            let effect = (!thread.workspace_id.is_empty()).then(|| Effect::Archive {
                machine: thread.machine_id.clone(),
                thread: thread.id.clone(),
                workspace_id: thread.workspace_id.clone(),
                title: thread.title.clone(),
            });
            app.archiving.insert(id.clone());
            effects.extend(effect);
            if failed_launch {
                // Archiving a failed launch also forgets it.
                app.dismiss(&id, effects);
            }
        }
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => effects.push(Effect::Quit),
        KeyCode::Char('q') => effects.push(Effect::Quit),
        KeyCode::Char('j') | KeyCode::Down => move_cursor(app, 1, now, effects),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(app, -1, now, effects),
        KeyCode::Char('g') | KeyCode::Home => select(app, 0, now, effects),
        KeyCode::Char('G') | KeyCode::End => select(app, app.threads.len().saturating_sub(1), now, effects),
        KeyCode::PageUp | KeyCode::PageDown => {
            let step = (app.layout.list.height / (super::layout::THREAD_LINES + 1)).max(1) as isize;
            move_cursor(app, if key.code == KeyCode::PageUp { -step } else { step }, now, effects);
        }
        KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') | KeyCode::Right => {
            if let Some(id) = app.cursor.clone() {
                focus_thread(app, &id, now, effects);
            }
        }
        KeyCode::Esc if !app.filter.is_empty() => {
            app.filter.clear();
            app.rebuild_now();
        }
        KeyCode::Tab | KeyCode::Esc => {
            if let Some(id) = app.open.as_ref().map(|open| open.id.clone()) {
                app.mark_seen(&id, now);
                app.focus = Focus::Terminal;
            }
        }
        KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
            app.confirm_archive = app.cursor.clone();
        }
        KeyCode::Char('n') => app.open_composer(now, effects),
        KeyCode::Char('/') => {
            app.filtering = true;
        }
        KeyCode::Char('r') => start_reply(app, now),
        KeyCode::Char('e') => {
            if let Some(id) = app.cursor.clone() {
                app.resend(&id, now, effects);
            }
        }
        KeyCode::Char('d') => {
            if let Some(id) = app.cursor.clone() {
                app.dismiss(&id, effects);
            }
        }
        // With saved machines the list stays up without a local server; `s`
        // starts one.
        KeyCode::Char('s') if app.local().connection == Connection::NoServer => app.start_local_server(now, effects),
        _ => {}
    }
}

/// Typing the list filter: every key edits it, Enter keeps it, Esc clears it.
fn filter_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.filter.clear();
            app.filtering = false;
        }
        KeyCode::Enter | KeyCode::Down | KeyCode::Up => app.filtering = false,
        KeyCode::Backspace => {
            if app.filter.pop().is_none() {
                app.filtering = false;
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => app.filter.push(c),
        _ => return,
    }
    app.rebuild_now();
}

/// `r`: write to the cursor's agent without opening its thread.
fn start_reply(app: &mut App, now: SystemTime) {
    let Some(thread) = app.cursor.as_deref().and_then(|id| app.thread(id)).cloned() else {
        return;
    };
    if thread.pane_id.is_empty() || thread.id.starts_with(super::LAUNCH_PREFIX) {
        app.notify("This launch never started an agent. Press e to send it again.", super::NoticeKind::Error, now);
        return;
    }
    if thread.status == crate::herdr::types::AgentStatus::Blocked {
        app.notify(
            format!("“{}” is waiting for an answer. Open it to reply.", thread.title),
            super::NoticeKind::Error,
            now,
        );
        return;
    }
    app.reply = Some(super::Reply::new(thread.id));
}

fn reply_key(app: &mut App, key: KeyEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    let Some(reply) = app.reply.as_mut() else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let newline = key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => app.reply = None,
        KeyCode::Char('c') if ctrl => app.reply = None,
        KeyCode::Enter if newline => reply.editor.insert("\n"),
        KeyCode::Enter => {
            if reply.editor.is_blank() {
                return;
            }
            let text = reply.message();
            let id = reply.thread.clone();
            app.reply = None;
            if let Some(thread) = app.thread(&id).cloned() {
                app.warn_if_images_stay_here(&thread.machine_id, &text, now);
                effects.push(Effect::Prompt {
                    machine: thread.machine_id,
                    pane_id: thread.pane_id,
                    title: thread.title,
                    text,
                });
            }
        }
        KeyCode::Char('w') if ctrl => reply.editor.delete_word(),
        KeyCode::Char('u') if ctrl => reply.editor.clear(),
        KeyCode::Char('v' | 'V') if ctrl => effects.push(Effect::ReadClipboard),
        KeyCode::Char(c) if !ctrl => {
            let mut buffer = [0; 4];
            reply.editor.insert(c.encode_utf8(&mut buffer));
        }
        KeyCode::Backspace => reply.editor.backspace(),
        KeyCode::Delete => reply.editor.delete(),
        KeyCode::Left => reply.editor.left(),
        KeyCode::Right => reply.editor.right(),
        KeyCode::Home => reply.editor.home(),
        KeyCode::End => reply.editor.end(),
        KeyCode::Up => {
            reply.editor.up();
        }
        KeyCode::Down => {
            reply.editor.down();
        }
        _ => {}
    }
}

/// Opens a thread and gives it the keyboard, if it actually opened.
fn focus_thread(app: &mut App, id: &str, now: SystemTime, effects: &mut Vec<Effect>) {
    app.open_thread(id, now, effects);
    if app.open.as_ref().is_some_and(|o| o.id == id) {
        app.focus = Focus::Terminal;
    }
}

fn move_cursor(app: &mut App, delta: isize, now: SystemTime, effects: &mut Vec<Effect>) {
    let Some(current) = app.cursor_index() else {
        if !app.threads.is_empty() {
            select(app, 0, now, effects);
        }
        return;
    };
    let last = app.threads.len().saturating_sub(1);
    select(app, current.saturating_add_signed(delta).min(last), now, effects);
}

fn select(app: &mut App, index: usize, now: SystemTime, effects: &mut Vec<Effect>) {
    app.layout.follow_cursor();
    if let Some(id) = app.threads.get(index).map(|thread| thread.id.clone()) {
        app.cursor = Some(id.clone());
        if app.open.as_ref().is_none_or(|open| open.id != id) {
            app.show_thread(&id, now, effects);
        }
    }
}

pub(super) fn paste(app: &mut App, text: &str, effects: &mut Vec<Effect>) {
    if app.updates.visible {
        return;
    }
    if let Some(reply) = app.reply.as_mut() {
        reply.editor.insert(text);
        return;
    }
    if app.filtering {
        app.filter.push_str(text.lines().next().unwrap_or(""));
        app.rebuild_now();
        return;
    }
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

/// The clipboard read for Ctrl+V: an image joins the reply or the task being
/// written as `[Image #N]`, text is pasted there like any paste.
pub(super) fn clipboard(app: &mut App, result: Result<Clip, String>, now: SystemTime, effects: &mut Vec<Effect>) {
    if app.updates.visible {
        return;
    }
    let writing_task =
        app.focus == Focus::Composer && app.composer.picker.is_none() && app.composer.field == Field::Task;
    if app.reply.is_none() && !writing_task {
        return;
    }
    match result {
        Ok(Clip::Image(path)) => match app.reply.as_mut() {
            Some(reply) => images::attach(&mut reply.editor, &mut reply.images, path),
            None => images::attach(&mut app.composer.task, &mut app.composer.images, path),
        },
        Ok(Clip::Text(text)) => paste(app, &text, effects),
        Ok(Clip::Empty) => app.notify("The clipboard is empty.", NoticeKind::Info, now),
        Err(error) => app.notify(error, NoticeKind::Error, now),
    }
}

pub(super) fn mouse(app: &mut App, mouse: MouseEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    if app.updates.visible || app.menu.is_some() || app.dictation.is_some() || app.reply.is_some() {
        return;
    }
    let (x, y) = (mouse.column, mouse.row);
    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && app.layout.on_new_button(x, y)
        && !app.needs_server_screen()
    {
        return app.open_composer(now, effects);
    }
    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && super::composer::layout::contains(app.layout.search, x, y)
        && !app.needs_server_screen()
    {
        app.focus = Focus::List;
        app.confirm_archive = None;
        app.filtering = true;
        return;
    }
    if app.focus == Focus::Composer && app.layout.in_terminal(x, y) {
        return composer_mouse(app, mouse, now, effects);
    }
    if app.layout.in_list(x, y) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                app.confirm_archive = None;
                if let Some(index) = app.layout.thread_at(x, y) {
                    select(app, index, now, effects);
                    if let Some(id) = app.cursor.clone() {
                        focus_thread(app, &id, now, effects);
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
            let id = open.id.clone();
            focus_thread(app, &id, now, effects);
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

fn composer_mouse(app: &mut App, mouse: MouseEvent, now: SystemTime, effects: &mut Vec<Effect>) {
    use super::composer::layout::{ComposerLayout, contains};
    let Some(layout) = ComposerLayout::new(app.layout.terminal, &app.composer, &app.composer_context()) else {
        return;
    };
    let (x, y) = (mouse.column, mouse.row);
    if let Some(popup) = &layout.picker
        && contains(popup.popup, x, y)
    {
        let Some(picker) = &app.composer.picker else { return };
        let choices = app.composer.choices_with_actions(&app.composer_context(), picker.field, &picker.query);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if contains(popup.rows, x, y) => {
                let index = popup.start + (y - popup.rows.y) as usize;
                if let Some(choice) = choices.get(index).filter(|c| c.enabled) {
                    choose(app, choice.pick.clone(), effects);
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if mouse.kind == MouseEventKind::ScrollDown {
                    WHEEL_LINES as isize
                } else {
                    -(WHEEL_LINES as isize)
                };
                if let Some(picker) = &mut app.composer.picker {
                    picker.selected = picker.selected.saturating_add_signed(delta).min(choices.len().saturating_sub(1));
                }
            }
            _ => {}
        }
        return;
    }
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    app.composer.close_picker();
    if contains(layout.task_box, x, y) {
        app.composer.field = Field::Task;
        app.composer.task.place_cursor(
            layout.task_inner.width,
            y.saturating_sub(layout.task_inner.y).min(layout.task_inner.height - 1) + layout.task_scroll,
            x.saturating_sub(layout.task_inner.x),
        );
    } else if let Some(field) = layout.field_at(x, y) {
        if app.composer.fields(&app.composer_context()).contains(&field) {
            app.composer.field = field;
            app.composer.open_picker(field, "");
        }
    } else if contains(layout.send, x, y) {
        app.send(false, now, effects);
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
