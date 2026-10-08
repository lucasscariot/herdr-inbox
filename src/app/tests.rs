use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::*;
use crate::herdr::terminal::{Frame, MouseAction, MouseButton as PaneButton, ScrollDirection};
use crate::herdr::types::AgentInfo;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn agent(pane: &str, status: AgentStatus, title: &str) -> AgentInfo {
    let workspace = pane.split(':').next().unwrap_or(pane);
    AgentInfo {
        pane_id: pane.into(),
        workspace_id: workspace.into(),
        tab_id: format!("{workspace}:t1"),
        terminal_id: String::new(),
        agent_status: status,
        agent: Some("claude".into()),
        display_agent: None,
        name: None,
        title: None,
        terminal_title_stripped: Some(title.into()),
        cwd: None,
        foreground_cwd: None,
        tokens: Default::default(),
        state_change_seq: 0,
        focused: false,
    }
}

fn snapshot(agents: Vec<AgentInfo>) -> Input {
    Input::Snapshot(SessionSnapshot {
        agents,
        ..SessionSnapshot::default()
    })
}

fn app() -> App {
    App::new(120, 40, Box::new(|_| None))
}

fn press(code: KeyCode) -> Input {
    press_with(code, KeyModifiers::NONE)
}

fn press_with(code: KeyCode, modifiers: KeyModifiers) -> Input {
    Input::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE })
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Input {
    Input::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
}

fn frame(generation: u64, bytes: &[u8]) -> Input {
    Input::Terminal {
        generation,
        message: Message::Frame(Frame { seq: 1, width: 79, height: 39, full: true, bytes: bytes.to_vec() }),
    }
}

/// An app showing three threads: blocked `w1:p1`, working `w2:p1`, idle `w3:p1`.
fn loaded() -> (App, Vec<Effect>) {
    let mut app = app();
    let effects = app.update(
        snapshot(vec![
            agent("w2:p1", AgentStatus::Working, "Build"),
            agent("w3:p1", AgentStatus::Idle, "Docs"),
            agent("w1:p1", AgentStatus::Blocked, "Login"),
        ]),
        at(0),
    );
    (app, effects)
}

fn ids(app: &App) -> Vec<&str> {
    app.threads.iter().map(|t| t.id.as_str()).collect()
}

#[test]
fn the_first_snapshot_opens_the_most_urgent_thread_without_stealing_focus() {
    let (app, effects) = loaded();
    assert_eq!(ids(&app), ["w1:p1", "w2:p1", "w3:p1"]);
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"));
    assert_eq!(effects, vec![Effect::Attach { generation: 1, pane_id: "w1:p1".into(), cols: 79, rows: 39 }]);
    assert_eq!(app.focus, Focus::List);
    assert_eq!(app.connection, Connection::Live);
}

#[test]
fn an_empty_first_snapshot_attaches_nothing() {
    let mut app = app();
    assert!(app.update(snapshot(vec![]), at(0)).is_empty());
    assert_eq!(app.cursor, None);
    assert!(app.open.is_none());
}

#[test]
fn later_snapshots_do_not_reattach() {
    let (mut app, _) = loaded();
    let effects = app.update(snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login")]), at(1));
    assert!(effects.is_empty());
}

#[test]
fn the_cursor_follows_its_thread_when_the_order_changes() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('j')), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w2:p1"));
    app.update(Input::Status(AgentStatusChange {
        pane_id: "w2:p1".into(),
        agent_status: AgentStatus::Blocked,
        agent: None,
        display_agent: None,
        title: None,
    }), at(2));
    assert_eq!(ids(&app)[0], "w2:p1", "the newest blocked thread moves to the top");
    assert_eq!(app.cursor.as_deref(), Some("w2:p1"));
}

#[test]
fn when_the_cursor_thread_vanishes_the_cursor_lands_on_its_neighbour() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('j')), at(1));
    app.update(
        snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login"), agent("w3:p1", AgentStatus::Idle, "Docs")]),
        at(2),
    );
    assert_eq!(app.cursor.as_deref(), Some("w3:p1"));
    app.update(snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login")]), at(3));
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"), "clamped to the last thread");
}

#[test]
fn the_open_thread_ending_detaches_and_says_so() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    assert_eq!(app.focus, Focus::Terminal);
    let effects = app.update(snapshot(vec![agent("w2:p1", AgentStatus::Working, "Build")]), at(2));
    assert_eq!(effects, vec![Effect::Detach]);
    assert!(app.open.is_none());
    assert_eq!(app.focus, Focus::List);
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("The thread you had open ended"));
}

#[test]
fn list_navigation_stays_in_bounds() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('k')), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"));
    app.update(press(KeyCode::Char('G')), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w3:p1"));
    app.update(press(KeyCode::Down), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w3:p1"));
    app.update(press(KeyCode::Char('g')), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"));
    app.update(press(KeyCode::End), at(1));
    app.update(press(KeyCode::Up), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w2:p1"));
}

#[test]
fn navigating_never_sends_keys_to_an_agent() {
    let (mut app, _) = loaded();
    for code in [KeyCode::Char('j'), KeyCode::Char('k'), KeyCode::Char('a'), KeyCode::Char('z'), KeyCode::F(3)] {
        assert!(app.update(press(code), at(1)).is_empty(), "{code:?}");
    }
}

#[test]
fn enter_switches_threads_with_a_new_generation() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('j')), at(1));
    let effects = app.update(press(KeyCode::Enter), at(1));
    assert_eq!(
        effects,
        vec![Effect::Detach, Effect::Attach { generation: 2, pane_id: "w2:p1".into(), cols: 79, rows: 39 }]
    );
    assert_eq!(app.focus, Focus::Terminal);
    assert_eq!(app.open.as_ref().map(|o| o.id.as_str()), Some("w2:p1"));
}

#[test]
fn opening_the_thread_already_streaming_only_moves_focus() {
    let (mut app, _) = loaded();
    app.update(frame(1, b"hello"), at(1));
    assert!(app.update(press(KeyCode::Enter), at(1)).is_empty());
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn keys_in_the_terminal_go_to_the_agent_except_tab() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    let effects = app.update(press(KeyCode::Char('q')), at(1));
    assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b"q".to_vec()) }]);
    let effects = app.update(press_with(KeyCode::BackTab, KeyModifiers::SHIFT), at(1));
    assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b"\x1b[Z".to_vec()) }]);
    let effects = app.update(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL), at(1));
    assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(vec![3]) }], "Ctrl+C interrupts the agent, not the inbox");
    assert!(app.update(press(KeyCode::Tab), at(1)).is_empty());
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn tab_back_to_the_list_puts_the_cursor_on_the_open_thread() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    app.focus = Focus::Terminal;
    app.cursor = Some("w3:p1".into());
    app.update(press(KeyCode::Tab), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"));
    app.update(press(KeyCode::Tab), at(1));
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn the_terminal_encodes_keys_with_the_panes_modes() {
    let (mut app, _) = loaded();
    app.update(frame(1, b"\x1b[?1h"), at(1));
    app.update(press(KeyCode::Enter), at(1));
    let effects = app.update(press(KeyCode::Up), at(1));
    assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b"\x1bOA".to_vec()) }]);
}

#[test]
fn frames_from_a_released_session_are_ignored() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('j')), at(1));
    app.update(press(KeyCode::Enter), at(1));
    app.update(frame(1, b"stale"), at(1));
    let open = app.open.as_ref().unwrap();
    assert_eq!(open.stream, StreamState::Attaching);
    assert!(!open.screen.has_content());
    app.update(frame(2, b"fresh"), at(1));
    let open = app.open.as_ref().unwrap();
    assert_eq!(open.stream, StreamState::Live);
    assert!(open.screen.text().starts_with("fresh"));
}

#[test]
fn a_taken_over_stream_can_be_taken_back_with_enter() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    app.update(Input::Terminal { generation: 1, message: Message::Closed { reason: Some("taken over".into()) } }, at(2));
    assert_eq!(app.open.as_ref().unwrap().stream, StreamState::Closed { reason: Some("taken over".into()) });
    assert_eq!(app.notice.as_ref().unwrap().kind, NoticeKind::Error);
    assert!(app.update(press(KeyCode::Char('x')), at(2)).is_empty(), "typing into a closed stream does nothing");
    let effects = app.update(press(KeyCode::Enter), at(2));
    assert_eq!(effects, vec![Effect::Detach, Effect::Attach { generation: 2, pane_id: "w1:p1".into(), cols: 79, rows: 39 }]);
}

#[test]
fn archiving_asks_first_and_cancels_on_anything_else() {
    let (mut app, _) = loaded();
    assert!(app.update(press(KeyCode::Char('x')), at(1)).is_empty());
    assert_eq!(app.confirm_archive.as_deref(), Some("w1:p1"));
    assert!(app.update(press(KeyCode::Char('j')), at(1)).is_empty());
    assert_eq!(app.confirm_archive, None);
    assert_eq!(app.cursor.as_deref(), Some("w1:p1"), "the cancelling key does nothing else");
    app.update(press(KeyCode::Delete), at(1));
    let effects = app.update(press(KeyCode::Char('y')), at(1));
    assert_eq!(
        effects,
        vec![Effect::Archive { thread: "w1:p1".into(), workspace_id: "w1".into(), title: "Login".into() }]
    );
    app.update(press(KeyCode::Backspace), at(1));
    assert_eq!(app.update(press(KeyCode::Enter), at(1)).len(), 1, "Enter confirms too");
}

#[test]
fn a_pending_archive_is_dropped_when_its_thread_ends() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('x')), at(1));
    app.update(snapshot(vec![agent("w2:p1", AgentStatus::Working, "Build")]), at(2));
    assert_eq!(app.confirm_archive, None);
}

#[test]
fn archive_results_are_reported_and_notices_expire() {
    let (mut app, _) = loaded();
    app.update(Input::Archived { title: "Login".into(), result: Ok(()) }, at(10));
    assert_eq!(app.notice.as_ref().unwrap().text, "Archived “Login”");
    app.update(Input::Tick, at(13));
    assert!(app.notice.is_some());
    app.update(Input::Tick, at(14));
    assert!(app.notice.is_none());
    app.update(Input::Archived { title: "Login".into(), result: Err("no such workspace".into()) }, at(20));
    let notice = app.notice.as_ref().unwrap();
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(notice.text.contains("no such workspace"));
}

#[test]
fn q_quits_from_the_list_and_ctrl_c_too() {
    let (mut app, _) = loaded();
    assert_eq!(app.update(press(KeyCode::Char('q')), at(1)), vec![Effect::Quit]);
    assert_eq!(app.update(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL), at(1)), vec![Effect::Quit]);
}

#[test]
fn without_a_server_enter_starts_one_and_q_quits() {
    let mut app = app();
    app.update(Input::Connection(Connection::NoServer), at(0));
    assert_eq!(app.update(press(KeyCode::Char('j')), at(0)), vec![]);
    assert_eq!(app.update(press(KeyCode::Enter), at(0)), vec![Effect::StartServer]);
    assert_eq!(app.connection, Connection::Starting);
    let mut app = self::app();
    app.update(Input::Connection(Connection::NoServer), at(0));
    assert_eq!(app.update(press(KeyCode::Char('q')), at(0)), vec![Effect::Quit]);
}

#[test]
fn losing_the_server_clears_threads_and_detaches() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    let effects = app.update(Input::Connection(Connection::Lost("socket closed".into())), at(2));
    assert_eq!(effects, vec![Effect::Detach]);
    assert!(app.threads.is_empty());
    assert_eq!(app.cursor, None);
    assert_eq!(app.focus, Focus::List);
    let effects = app.update(snapshot(vec![agent("w1:p1", AgentStatus::Idle, "Login")]), at(3));
    assert_eq!(effects.len(), 1, "coming back re-attaches like a first start");
}

#[test]
fn resizing_the_window_resizes_the_open_pane_once() {
    let (mut app, _) = loaded();
    let effects = app.update(Input::Resize { width: 150, height: 50 }, at(1));
    let (cols, rows) = app.layout.terminal_size();
    assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Resize { cols, rows } }]);
    assert!(app.update(Input::Resize { width: 150, height: 50 }, at(1)).is_empty());
}

#[test]
fn resizing_without_an_open_thread_only_updates_the_layout() {
    let mut app = app();
    assert!(app.update(Input::Resize { width: 80, height: 20 }, at(0)).is_empty());
    assert_eq!(app.layout.width, 80);
}

#[test]
fn status_changes_update_status_title_and_age() {
    let (mut app, _) = loaded();
    app.update(Input::Status(AgentStatusChange {
        pane_id: "w3:p1".into(),
        agent_status: AgentStatus::Working,
        agent: Some("codex".into()),
        display_agent: None,
        title: Some("ignored while a terminal title exists".into()),
    }), at(5));
    let thread = app.thread("w3:p1").unwrap();
    assert_eq!(thread.status, AgentStatus::Working);
    assert_eq!(thread.harness, "Codex");
    assert_eq!(thread.title, "Docs");
    assert_eq!(thread.changed_at, Some(at(5)));
}

#[test]
fn status_changes_for_unknown_panes_or_before_a_snapshot_are_ignored() {
    let change = AgentStatusChange {
        pane_id: "w9:p9".into(),
        agent_status: AgentStatus::Done,
        agent: None,
        display_agent: None,
        title: None,
    };
    let mut fresh = app();
    fresh.update(Input::Status(change.clone()), at(0));
    assert!(fresh.threads.is_empty());
    let (mut app, _) = loaded();
    let before = app.threads.clone();
    app.update(Input::Status(change), at(1));
    assert_eq!(app.threads, before);
}

fn finish(app: &mut App, pane: &str, secs: u64) {
    for status in [AgentStatus::Working, AgentStatus::Done] {
        app.update(
            Input::Status(AgentStatusChange {
                pane_id: pane.into(),
                agent_status: status,
                agent: None,
                display_agent: None,
                title: None,
            }),
            at(secs),
        );
    }
}

#[test]
fn a_thread_finishing_while_watched_is_not_flagged_ready() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    finish(&mut app, "w1:p1", 2);
    assert_eq!(app.thread("w1:p1").unwrap().status, AgentStatus::Idle);
}

#[test]
fn a_thread_finishing_in_the_background_is_ready_until_opened() {
    let (mut app, _) = loaded();
    finish(&mut app, "w3:p1", 2);
    assert_eq!(app.thread("w3:p1").unwrap().status, AgentStatus::Done);
    app.cursor = Some("w3:p1".into());
    app.update(press(KeyCode::Enter), at(3));
    assert_eq!(app.thread("w3:p1").unwrap().status, AgentStatus::Idle);
}

#[test]
fn a_thread_finishing_while_its_terminal_is_open_but_unfocused_is_ready() {
    let (mut app, _) = loaded();
    // Opened at start, focus still on the list.
    finish(&mut app, "w1:p1", 2);
    assert_eq!(app.thread("w1:p1").unwrap().status, AgentStatus::Done);
}

#[test]
fn threads_appearing_later_get_an_age_but_initial_ones_do_not() {
    let (mut app, _) = loaded();
    assert!(app.threads.iter().all(|t| t.changed_at.is_none()));
    app.update(
        snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login"), agent("w4:p1", AgentStatus::Working, "New")]),
        at(7),
    );
    assert_eq!(app.thread("w4:p1").unwrap().changed_at, Some(at(7)));
    assert_eq!(app.thread("w1:p1").unwrap().changed_at, None);
}

#[test]
fn paste_goes_to_the_agent_only_from_the_terminal() {
    let (mut app, _) = loaded();
    assert!(app.update(Input::Paste("ls".into()), at(1)).is_empty());
    app.update(frame(1, b"\x1b[?2004h"), at(1));
    app.update(press(KeyCode::Enter), at(1));
    assert_eq!(
        app.update(Input::Paste("a\nb".into()), at(1)),
        vec![Effect::Send { generation: 1, control: Control::Input(b"\x1b[200~a\rb\x1b[201~".to_vec()) }]
    );
}

#[test]
fn clicking_a_thread_opens_and_focuses_it() {
    let (mut app, _) = loaded();
    let y = app.layout.list.y + 6; // Heading, w1 (3 lines), blank, heading, then w2.
    let effects = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 2, y), at(1));
    assert_eq!(app.cursor.as_deref(), Some("w2:p1"));
    assert_eq!(effects.last(), Some(&Effect::Attach { generation: 2, pane_id: "w2:p1".into(), cols: 79, rows: 39 }));
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn clicking_empty_list_space_changes_nothing() {
    let (mut app, _) = loaded();
    let effects = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 2, app.layout.list.y), at(1));
    assert!(effects.is_empty());
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn the_wheel_scrolls_the_pane_and_clicks_reach_apps_that_want_them() {
    let (mut app, _) = loaded();
    let x = app.layout.terminal.x + 4;
    let y = app.layout.terminal.y + 2;
    assert_eq!(
        app.update(mouse(MouseEventKind::ScrollUp, x, y), at(1)),
        vec![Effect::Send { generation: 1, control: Control::Scroll { direction: ScrollDirection::Up, lines: 3 } }]
    );
    assert!(app.update(mouse(MouseEventKind::Down(MouseButton::Left), x, y), at(1)).is_empty());
    assert_eq!(app.focus, Focus::Terminal, "a click focuses the terminal");
    app.update(frame(1, b"\x1b[?1000h"), at(1));
    assert_eq!(
        app.update(mouse(MouseEventKind::Down(MouseButton::Left), x, y), at(1)),
        vec![Effect::Send {
            generation: 1,
            control: Control::Mouse { action: MouseAction::Down, button: PaneButton::Left, column: 4, row: 2, modifiers: 0 },
        }]
    );
}

#[test]
fn mouse_modifiers_use_herdr_bits() {
    let (mut app, _) = loaded();
    app.update(frame(1, b"\x1b[?1000h"), at(1));
    let mut event = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Right),
        column: app.layout.terminal.x,
        row: 0,
        modifiers: KeyModifiers::SHIFT | KeyModifiers::ALT,
    };
    let effects = app.update(Input::Mouse(event), at(1));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Send { control: Control::Mouse { action: MouseAction::Drag, button: PaneButton::Right, modifiers: 5, .. }, .. }]
    ));
    event.modifiers = KeyModifiers::CONTROL;
    event.kind = MouseEventKind::Up(MouseButton::Middle);
    let effects = app.update(Input::Mouse(event), at(1));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Send { control: Control::Mouse { action: MouseAction::Up, button: PaneButton::Middle, modifiers: 2, .. }, .. }]
    ));
}

#[test]
fn the_separator_and_status_bar_ignore_the_mouse() {
    let (mut app, _) = loaded();
    let separator = app.layout.sidebar.width;
    assert!(app.update(mouse(MouseEventKind::Down(MouseButton::Left), separator, 5), at(1)).is_empty());
    assert!(app.update(mouse(MouseEventKind::Down(MouseButton::Left), 50, app.layout.bar.y), at(1)).is_empty());
}

#[test]
fn key_releases_are_ignored_everywhere() {
    let (mut app, _) = loaded();
    let release = Input::Key(KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    });
    assert!(app.update(release, at(1)).is_empty());
}
