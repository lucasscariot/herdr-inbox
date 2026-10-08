use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::*;
use crate::herdr::terminal::{Frame, MouseAction, MouseButton as PaneButton, ScrollDirection};
use crate::herdr::types::AgentInfo;
use crate::threads::LOCAL;

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
    snapshot_on(LOCAL, agents)
}

fn snapshot_on(machine: &str, agents: Vec<AgentInfo>) -> Input {
    Input::Snapshot {
        machine: machine.into(),
        snapshot: SessionSnapshot { agents, ..SessionSnapshot::default() },
        checkouts: Default::default(),
    }
}

fn status(pane: &str, agent_status: AgentStatus) -> Input {
    status_on(LOCAL, pane, agent_status)
}

fn status_on(machine: &str, pane: &str, agent_status: AgentStatus) -> Input {
    Input::Status {
        machine: machine.into(),
        change: AgentStatusChange { pane_id: pane.into(), agent_status, agent: None, display_agent: None, title: None },
    }
}

fn connection(machine: &str, connection: Connection) -> Input {
    Input::Connection { machine: machine.into(), connection }
}

/// `local/<pane>`.
fn l(pane: &str) -> String {
    threads::thread_id(LOCAL, pane)
}

fn app() -> App {
    App::new(120, 40, vec![])
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

/// Pane ids of the listed threads, in order.
fn ids(app: &App) -> Vec<&str> {
    app.threads.iter().map(|t| t.pane_id.as_str()).collect()
}

#[test]
fn the_first_snapshot_opens_the_most_urgent_thread_without_stealing_focus() {
    let (app, effects) = loaded();
    assert_eq!(ids(&app), ["w1:p1", "w2:p1", "w3:p1"]);
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()));
    assert_eq!(
        effects,
        vec![Effect::Attach { generation: 1, machine: LOCAL.into(), pane_id: "w1:p1".into(), cols: 79, rows: 39 }]
    );
    assert_eq!(app.focus, Focus::List);
    assert_eq!(app.local().connection, Connection::Live);
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
    assert_eq!(app.cursor.as_deref(), Some(l("w2:p1").as_str()));
    app.update(
        Input::Status {
            machine: LOCAL.into(),
            change: AgentStatusChange {
                pane_id: "w2:p1".into(),
                agent_status: AgentStatus::Blocked,
                agent: None,
                display_agent: None,
                title: None,
            },
        },
        at(2),
    );
    assert_eq!(ids(&app)[0], "w2:p1", "the newest blocked thread moves to the top");
    assert_eq!(app.cursor.as_deref(), Some(l("w2:p1").as_str()));
}

#[test]
fn when_the_cursor_thread_vanishes_the_cursor_lands_on_its_neighbour() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Char('j')), at(1));
    app.update(
        snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login"), agent("w3:p1", AgentStatus::Idle, "Docs")]),
        at(2),
    );
    assert_eq!(app.cursor.as_deref(), Some(l("w3:p1").as_str()));
    app.update(snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login")]), at(3));
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()), "clamped to the last thread");
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
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()));
    app.update(press(KeyCode::Char('G')), at(1));
    assert_eq!(app.cursor.as_deref(), Some(l("w3:p1").as_str()));
    app.update(press(KeyCode::Down), at(1));
    assert_eq!(app.cursor.as_deref(), Some(l("w3:p1").as_str()));
    app.update(press(KeyCode::Char('g')), at(1));
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()));
    app.update(press(KeyCode::End), at(1));
    app.update(press(KeyCode::Up), at(1));
    assert_eq!(app.cursor.as_deref(), Some(l("w2:p1").as_str()));
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
        vec![
            Effect::Detach,
            Effect::Attach { generation: 2, machine: LOCAL.into(), pane_id: "w2:p1".into(), cols: 79, rows: 39 }
        ]
    );
    assert_eq!(app.focus, Focus::Terminal);
    assert_eq!(app.open.as_ref().map(|o| o.id.as_str()), Some(l("w2:p1").as_str()));
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
    assert_eq!(
        effects,
        vec![Effect::Send { generation: 1, control: Control::Input(vec![3]) }],
        "Ctrl+C interrupts the agent, not the inbox"
    );
    assert!(app.update(press(KeyCode::Tab), at(1)).is_empty());
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn tab_back_to_the_list_puts_the_cursor_on_the_open_thread() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    app.focus = Focus::Terminal;
    app.cursor = Some(l("w3:p1"));
    app.update(press(KeyCode::Tab), at(1));
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()));
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
    app.update(Input::Terminal { generation: 1, message: Message::Closed { reason: Some(TAKEN_OVER.into()) } }, at(2));
    assert_eq!(app.open.as_ref().unwrap().stream, StreamState::Closed { reason: Some(TAKEN_OVER.into()) });
    assert!(app.notice.is_none(), "the terminal area explains it; no second message");
    assert!(app.update(press(KeyCode::Char('x')), at(2)).is_empty(), "typing into a closed stream does nothing");
    let effects = app.update(press(KeyCode::Enter), at(2));
    assert_eq!(
        effects,
        vec![
            Effect::Detach,
            Effect::Attach { generation: 2, machine: LOCAL.into(), pane_id: "w1:p1".into(), cols: 79, rows: 39 }
        ]
    );
}

#[test]
fn archiving_asks_first_and_cancels_on_anything_else() {
    let (mut app, _) = loaded();
    assert!(app.update(press(KeyCode::Char('x')), at(1)).is_empty());
    assert_eq!(app.confirm_archive.as_deref(), Some(l("w1:p1").as_str()));
    assert!(app.update(press(KeyCode::Char('j')), at(1)).is_empty());
    assert_eq!(app.confirm_archive, None);
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()), "the cancelling key does nothing else");
    app.update(press(KeyCode::Delete), at(1));
    let effects = app.update(press(KeyCode::Char('y')), at(1));
    assert_eq!(
        effects,
        vec![Effect::Archive {
            machine: LOCAL.into(),
            thread: l("w1:p1"),
            workspace_id: "w1".into(),
            title: "Login".into()
        }]
    );
    app.update(press(KeyCode::Backspace), at(1));
    assert_eq!(app.update(press(KeyCode::Enter), at(1)).len(), 1, "Enter confirms too");
}

#[test]
fn archiving_the_open_thread_reports_the_archive_whichever_message_comes_first() {
    for archived_first in [true, false] {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Enter), at(1));
        app.update(press(KeyCode::Tab), at(1));
        app.update(press(KeyCode::Char('x')), at(1));
        app.update(press(KeyCode::Char('y')), at(1));
        let archived = Input::Archived { title: "Login".into(), result: Ok(()) };
        let gone =
            snapshot(vec![agent("w2:p1", AgentStatus::Working, "Build"), agent("w3:p1", AgentStatus::Idle, "Docs")]);
        if archived_first {
            app.update(archived, at(2));
            app.update(gone, at(2));
        } else {
            app.update(gone, at(2));
            app.update(archived, at(2));
        }
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.as_str()),
            Some("Archived “Login”"),
            "archived first: {archived_first}"
        );
        assert!(app.open.is_none());
    }
}

#[test]
fn a_thread_that_ends_on_its_own_is_still_reported() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    app.update(snapshot(vec![agent("w2:p1", AgentStatus::Working, "Build")]), at(2));
    assert_eq!(app.notice.as_ref().map(|n| n.text.as_str()), Some("The thread you had open ended"));
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
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    assert_eq!(app.update(press(KeyCode::Char('j')), at(0)), vec![]);
    assert_eq!(app.update(press(KeyCode::Enter), at(0)), vec![Effect::StartServer]);
    assert_eq!(app.local().connection, Connection::Starting(at(0)));
    let mut app = self::app();
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    assert_eq!(app.update(press(KeyCode::Char('q')), at(0)), vec![Effect::Quit]);
}

#[test]
fn a_starting_server_is_given_time_before_giving_up() {
    let mut app = app();
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    app.update(press(KeyCode::Enter), at(0));
    app.update(connection(LOCAL, Connection::NoServer), at(9));
    assert_eq!(app.local().connection, Connection::Starting(at(0)), "still booting");
    assert!(app.update(press(KeyCode::Enter), at(9)).is_empty(), "no second server");
    app.update(connection(LOCAL, Connection::NoServer), at(10));
    assert_eq!(app.local().connection, Connection::NoServer);
    assert_eq!(app.notice.as_ref().unwrap().kind, NoticeKind::Error);
    assert_eq!(app.update(press(KeyCode::Enter), at(11)), vec![Effect::StartServer], "the user can try again");
    assert!(app.notice.is_none(), "trying again clears the failure");
}

#[test]
fn a_started_server_takes_over_from_the_starting_screen() {
    let mut app = app();
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    app.update(press(KeyCode::Enter), at(0));
    app.update(snapshot(vec![agent("w1:p1", AgentStatus::Idle, "Login")]), at(2));
    assert_eq!(app.local().connection, Connection::Live);
    assert_eq!(app.threads.len(), 1);
}

#[test]
fn q_quits_while_a_server_starts() {
    let mut app = app();
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    app.update(press(KeyCode::Enter), at(0));
    assert_eq!(app.update(press(KeyCode::Char('q')), at(1)), vec![Effect::Quit]);
}

#[test]
fn losing_the_server_clears_threads_and_detaches() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    let effects = app.update(connection(LOCAL, Connection::Lost("socket closed".into())), at(2));
    assert_eq!(effects, vec![Effect::Detach]);
    assert!(app.threads.is_empty());
    assert_eq!(app.cursor, None);
    assert_eq!(app.focus, Focus::List);
    let effects = app.update(
        snapshot(vec![agent("w2:p1", AgentStatus::Blocked, "Build"), agent("w1:p1", AgentStatus::Idle, "Login")]),
        at(3),
    );
    assert_eq!(
        effects,
        vec![Effect::Attach { generation: 2, machine: LOCAL.into(), pane_id: "w1:p1".into(), cols: 79, rows: 39 }],
        "coming back re-opens the thread that was open, not the top one"
    );
    assert_eq!(app.cursor.as_deref(), Some(l("w1:p1").as_str()));
}

#[test]
fn a_resumed_thread_that_did_not_come_back_opens_nothing() {
    let (mut app, _) = loaded();
    app.update(connection(LOCAL, Connection::Lost("socket closed".into())), at(2));
    let effects = app.update(snapshot(vec![agent("w2:p1", AgentStatus::Blocked, "Build")]), at(3));
    assert!(effects.is_empty(), "the first start already auto-opened once");
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
    app.update(
        Input::Status {
            machine: LOCAL.into(),
            change: AgentStatusChange {
                pane_id: "w3:p1".into(),
                agent_status: AgentStatus::Working,
                agent: Some("codex".into()),
                display_agent: None,
                title: Some("ignored while a terminal title exists".into()),
            },
        },
        at(5),
    );
    let thread = app.thread(&l("w3:p1")).unwrap();
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
    fresh.update(Input::Status { machine: LOCAL.into(), change: change.clone() }, at(0));
    assert!(fresh.threads.is_empty());
    let (mut app, _) = loaded();
    let before = app.threads.clone();
    app.update(Input::Status { machine: LOCAL.into(), change }, at(1));
    assert_eq!(app.threads, before);
}

fn finish(app: &mut App, pane: &str, secs: u64) {
    app.update(status(pane, AgentStatus::Working), at(secs));
    app.update(status(pane, AgentStatus::Done), at(secs));
}

#[test]
fn a_thread_finishing_while_watched_is_not_flagged_ready() {
    let (mut app, _) = loaded();
    app.update(press(KeyCode::Enter), at(1));
    finish(&mut app, "w1:p1", 2);
    assert_eq!(app.thread(&l("w1:p1")).unwrap().status, AgentStatus::Idle);
}

#[test]
fn a_thread_finishing_in_the_background_is_ready_until_opened() {
    let (mut app, _) = loaded();
    finish(&mut app, "w3:p1", 2);
    assert_eq!(app.thread(&l("w3:p1")).unwrap().status, AgentStatus::Done);
    app.cursor = Some(l("w3:p1"));
    app.update(press(KeyCode::Enter), at(3));
    assert_eq!(app.thread(&l("w3:p1")).unwrap().status, AgentStatus::Idle);
}

#[test]
fn a_thread_finishing_while_its_terminal_is_open_but_unfocused_is_ready() {
    let (mut app, _) = loaded();
    // Opened at start, focus still on the list.
    finish(&mut app, "w1:p1", 2);
    assert_eq!(app.thread(&l("w1:p1")).unwrap().status, AgentStatus::Done);
}

#[test]
fn threads_appearing_later_get_an_age_but_initial_ones_do_not() {
    let (mut app, _) = loaded();
    assert!(app.threads.iter().all(|t| t.changed_at.is_none()));
    app.update(
        snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Login"), agent("w4:p1", AgentStatus::Working, "New")]),
        at(7),
    );
    assert_eq!(app.thread(&l("w4:p1")).unwrap().changed_at, Some(at(7)));
    assert_eq!(app.thread(&l("w1:p1")).unwrap().changed_at, None);
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
    assert_eq!(app.cursor.as_deref(), Some(l("w2:p1").as_str()));
    assert_eq!(
        effects.last(),
        Some(&Effect::Attach { generation: 2, machine: LOCAL.into(), pane_id: "w2:p1".into(), cols: 79, rows: 39 })
    );
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
            control: Control::Mouse {
                action: MouseAction::Down,
                button: PaneButton::Left,
                column: 4,
                row: 2,
                modifiers: 0
            },
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
        [Effect::Send {
            control: Control::Mouse { action: MouseAction::Drag, button: PaneButton::Right, modifiers: 5, .. },
            ..
        }]
    ));
    event.modifiers = KeyModifiers::CONTROL;
    event.kind = MouseEventKind::Up(MouseButton::Middle);
    let effects = app.update(Input::Mouse(event), at(1));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Send {
            control: Control::Mouse { action: MouseAction::Up, button: PaneButton::Middle, modifiers: 2, .. },
            ..
        }]
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

fn studio() -> MachineInfo {
    MachineInfo { id: "studio".into(), label: "Mac Studio".into() }
}

/// Local has a blocked `w1:p1`; the studio has a working `w1:p1` and an idle `w2:p1`.
fn fleet() -> App {
    let mut app = App::new(120, 40, vec![studio()]);
    app.update(snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Local login")]), at(0));
    app.update(
        snapshot_on(
            "studio",
            vec![
                agent("w1:p1", AgentStatus::Working, "Studio build"),
                agent("w2:p1", AgentStatus::Idle, "Studio docs"),
            ],
        ),
        at(0),
    );
    app
}

#[test]
fn the_local_machine_always_comes_first_and_duplicates_are_ignored() {
    let app = App::new(80, 20, vec![MachineInfo::local(), studio()]);
    let labels: Vec<&str> = app.machines.iter().map(|m| m.label.as_str()).collect();
    assert_eq!(labels, ["Local", "Mac Studio"]);
    assert!(app.machines[0].is_local());
    assert!(!app.local_only());
}

#[test]
fn threads_from_every_machine_share_one_list() {
    let app = fleet();
    let ids: Vec<&str> = app.threads.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["local/w1:p1", "studio/w1:p1", "studio/w2:p1"]);
    assert_eq!(app.thread("studio/w1:p1").unwrap().machine_label.as_deref(), Some("Mac Studio"));
    assert_eq!(app.thread("local/w1:p1").unwrap().machine_label, None);
}

#[test]
fn a_status_change_applies_to_its_own_machine_only() {
    let mut app = fleet();
    app.update(status_on("studio", "w1:p1", AgentStatus::Blocked), at(5));
    assert_eq!(app.thread("studio/w1:p1").unwrap().status, AgentStatus::Blocked);
    assert_eq!(app.thread("local/w1:p1").unwrap().status, AgentStatus::Blocked);
    app.update(status_on("studio", "w2:p1", AgentStatus::Working), at(6));
    assert_eq!(app.thread("studio/w2:p1").unwrap().status, AgentStatus::Working);
    app.update(status_on("nowhere", "w2:p1", AgentStatus::Done), at(7));
    assert_eq!(app.thread("studio/w2:p1").unwrap().status, AgentStatus::Working, "unknown machines are ignored");
}

#[test]
fn opening_and_archiving_a_remote_thread_name_its_machine() {
    let mut app = fleet();
    app.update(press(KeyCode::Char('j')), at(1));
    assert_eq!(app.cursor.as_deref(), Some("studio/w1:p1"));
    let effects = app.update(press(KeyCode::Enter), at(1));
    assert_eq!(
        effects,
        vec![
            Effect::Detach,
            Effect::Attach { generation: 2, machine: "studio".into(), pane_id: "w1:p1".into(), cols: 79, rows: 39 }
        ]
    );
    app.update(press(KeyCode::Tab), at(1));
    app.update(press(KeyCode::Char('x')), at(1));
    assert_eq!(
        app.update(press(KeyCode::Char('y')), at(1)),
        vec![Effect::Archive {
            machine: "studio".into(),
            thread: "studio/w1:p1".into(),
            workspace_id: "w1".into(),
            title: "Studio build".into(),
        }]
    );
}

#[test]
fn an_unreachable_machine_takes_only_its_own_threads_away() {
    let mut app = fleet();
    app.update(press(KeyCode::Char('j')), at(1));
    app.update(press(KeyCode::Enter), at(1));
    let effects =
        app.update(connection("studio", Connection::Lost("ssh: connect to host studio: timed out".into())), at(2));
    assert_eq!(effects, vec![Effect::Detach]);
    let ids: Vec<&str> = app.threads.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["local/w1:p1"]);
    assert_eq!(app.cursor.as_deref(), Some("local/w1:p1"));
    assert!(app.open.is_none());
    assert_eq!(app.local().connection, Connection::Live);
    assert_eq!(app.overall_connection(), Connection::Live, "one live machine is enough");
}

#[test]
fn a_remote_thread_that_was_open_comes_back_with_its_machine() {
    let mut app = fleet();
    app.update(press(KeyCode::Char('j')), at(1));
    app.update(press(KeyCode::Enter), at(1));
    app.update(connection("studio", Connection::Lost("timed out".into())), at(2));
    app.update(snapshot(vec![agent("w1:p1", AgentStatus::Blocked, "Local login")]), at(3));
    assert!(app.open.is_none(), "another machine's snapshot does not consume the resume");
    let effects = app.update(snapshot_on("studio", vec![agent("w1:p1", AgentStatus::Working, "Studio build")]), at(4));
    assert_eq!(
        effects,
        vec![Effect::Attach { generation: 3, machine: "studio".into(), pane_id: "w1:p1".into(), cols: 79, rows: 39 }]
    );
}

#[test]
fn without_a_local_server_remote_threads_stay_usable_and_s_starts_one() {
    let mut app = App::new(120, 40, vec![studio()]);
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    app.update(snapshot_on("studio", vec![agent("w1:p1", AgentStatus::Working, "Studio build")]), at(0));
    assert!(!app.needs_server_screen());
    assert_eq!(app.threads.len(), 1);
    assert_eq!(app.update(press(KeyCode::Char('j')), at(1)), vec![], "the list works");
    assert_eq!(app.update(press(KeyCode::Char('s')), at(1)), vec![Effect::StartServer]);
    assert_eq!(app.local().connection, Connection::Starting(at(1)));
    assert!(app.update(press(KeyCode::Char('s')), at(2)).is_empty(), "s only starts a missing server");
}

#[test]
fn the_overall_connection_summarises_every_machine() {
    let mut app = App::new(120, 40, vec![studio()]);
    assert_eq!(app.overall_connection(), Connection::Connecting);
    app.update(connection(LOCAL, Connection::NoServer), at(0));
    assert_eq!(app.overall_connection(), Connection::Connecting, "the studio may still answer");
    app.update(connection("studio", Connection::Lost("refused".into())), at(0));
    assert!(matches!(app.overall_connection(), Connection::Lost(_)));
    app.update(snapshot_on("studio", vec![]), at(1));
    assert_eq!(app.overall_connection(), Connection::Live);
}
