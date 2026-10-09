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

mod composer_flow {
    use super::*;
    use crate::discovery::{CheckoutEntry, Inventory, Project};
    use crate::launch::{self, Outcome, Record, Stage};
    use std::collections::BTreeMap;

    fn inventory() -> Inventory {
        Inventory {
            projects: vec![Project {
                name: "cockpit".into(),
                path: "/w/cockpit".into(),
                branch: "main".into(),
                checkouts: vec![CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false }],
            }],
            harnesses: vec!["claude".into(), "codex".into()],
            models: BTreeMap::new(),
            models_at: 0,
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
    }

    /// The loaded app with the composer open and the local inventory in.
    fn composing() -> App {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('n')), at(1));
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory()) }, at(1));
        app
    }

    fn launch_of(effects: &[Effect]) -> Option<&Record> {
        effects.iter().find_map(|e| match e {
            Effect::Launch { plan, .. } => Some(&plan.record),
            _ => None,
        })
    }

    #[test]
    fn n_opens_the_composer_and_discovers_every_live_machine() {
        let (mut app, _) = loaded();
        let effects = app.update(press(KeyCode::Char('n')), at(1));
        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(effects, vec![Effect::Discover { machine: LOCAL.into(), include_models: true }]);
        assert!(app.update(press(KeyCode::Esc), at(1)).is_empty());
        app.update(press(KeyCode::Tab), at(1));
        let effects = app.update(press(KeyCode::Char('n')), at(1));
        assert!(effects.is_empty(), "no second discovery while one runs");
    }

    #[test]
    fn fresh_models_are_not_read_again_unless_asked() {
        let mut app = composing();
        let mut fresh = inventory();
        fresh.models.insert("claude".into(), Default::default());
        fresh.models_at = crate::discovery::seconds(at(1));
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(fresh) }, at(1));
        app.update(press(KeyCode::Esc), at(2));
        app.update(press(KeyCode::Tab), at(2));
        let effects = app.update(press(KeyCode::Char('n')), at(2));
        assert_eq!(effects, vec![Effect::Discover { machine: LOCAL.into(), include_models: false }]);
        app.update(Input::Inventory { machine: LOCAL.into(), result: Err("x".into()) }, at(2));
        let effects = app.update(press(KeyCode::F(5)), at(2));
        assert_eq!(effects, vec![Effect::Discover { machine: LOCAL.into(), include_models: true }], "F5 rescans fully");
    }

    #[test]
    fn a_failed_discovery_keeps_the_last_inventory_and_says_why() {
        let mut app = composing();
        app.update(Input::Inventory { machine: LOCAL.into(), result: Err("python3: not found".into()) }, at(2));
        assert_eq!(app.discovery_errors[LOCAL], "python3: not found");
        assert_eq!(app.composer.project.as_deref(), Some("cockpit"), "the previous inventory still works");
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory()) }, at(3));
        assert!(app.discovery_errors.is_empty());
    }

    #[test]
    fn the_inventory_settles_the_composers_choices() {
        let app = composing();
        assert_eq!(app.composer.project.as_deref(), Some("cockpit"));
        assert_eq!(app.composer.machine.as_deref(), Some(LOCAL));
        assert_eq!(app.composer.harness.as_deref(), Some("claude"));
    }

    #[test]
    fn enter_sends_clears_the_task_and_tracks_the_launch() {
        let mut app = composing();
        type_text(&mut app, "Fix login");
        let effects = app.update(press(KeyCode::Enter), at(5));
        let record = launch_of(&effects).expect("a launch");
        assert_eq!(record.title, "Fix login");
        assert_eq!(record.branch, "fix-login");
        assert_eq!(record.harness, "claude");
        assert!(effects.iter().any(|e| matches!(e, Effect::Launch { machine, .. } if machine == LOCAL)));
        assert!(effects.contains(&Effect::SaveHistory("Fix login".into())), "the task joins the history");
        assert_eq!(app.composer.task.text(), "");
        assert_eq!(app.launches.len(), 1);
        assert_eq!(app.launches[0].state, LaunchState::Running("starting".into()));
        assert_eq!(app.focus, Focus::Composer, "the composer stays open for the next task");
    }

    #[test]
    fn ctrl_s_sends_and_keeps_the_task_for_another_launch() {
        let mut app = composing();
        type_text(&mut app, "Same task twice");
        let first = app.update(press_with(KeyCode::Char('s'), KeyModifiers::CONTROL), at(5));
        let second = app.update(press_with(KeyCode::Enter, KeyModifiers::CONTROL), at(5));
        let (a, b) = (launch_of(&first).unwrap(), launch_of(&second).unwrap());
        assert_ne!(a.id, b.id, "every launch has its own id");
        assert_eq!(app.composer.task.text(), "Same task twice");
    }

    #[test]
    fn shift_enter_adds_a_line_instead_of_sending() {
        let mut app = composing();
        type_text(&mut app, "one");
        assert!(app.update(press_with(KeyCode::Enter, KeyModifiers::SHIFT), at(1)).is_empty());
        type_text(&mut app, "two");
        assert_eq!(app.composer.task.text(), "one\ntwo");
    }

    #[test]
    fn a_blank_task_is_refused_with_a_reason() {
        let mut app = composing();
        assert!(app.update(press(KeyCode::Enter), at(1)).is_empty());
        assert_eq!(app.composer.error.as_deref(), Some("Write a task first."));
        type_text(&mut app, "x");
        app.update(press(KeyCode::F(3)), at(1));
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.composer.error, None, "a new choice clears the error");
    }

    #[test]
    fn esc_returns_to_the_agent_or_the_list_and_keeps_the_draft() {
        let mut app = composing();
        type_text(&mut app, "draft");
        app.update(press(KeyCode::Esc), at(1));
        assert_eq!(app.focus, Focus::Terminal, "a thread is open, so back to it");
        app.update(press(KeyCode::Tab), at(1));
        app.update(press(KeyCode::Char('n')), at(1));
        assert_eq!(app.composer.task.text(), "draft");
    }

    #[test]
    fn ctrl_c_clears_the_task_then_closes() {
        let mut app = composing();
        type_text(&mut app, "draft");
        app.update(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL), at(1));
        assert_eq!(app.composer.task.text(), "");
        assert_eq!(app.focus, Focus::Composer);
        app.update(press_with(KeyCode::Char('c'), KeyModifiers::CONTROL), at(1));
        assert_ne!(app.focus, Focus::Composer);
    }

    #[test]
    fn pickers_filter_as_you_type_and_apply_on_enter() {
        let mut app = composing();
        app.update(press(KeyCode::F(3)), at(1));
        assert_eq!(app.composer.picker.as_ref().map(|p| p.field), Some(Field::Harness));
        type_text(&mut app, "cdx");
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.composer.harness.as_deref(), Some("codex"));
        assert!(app.composer.picker.is_none());
        app.update(press(KeyCode::F(3)), at(1));
        app.update(press(KeyCode::Down), at(1));
        app.update(press(KeyCode::Down), at(1));
        app.update(press(KeyCode::Down), at(1));
        assert_eq!(app.composer.picker.as_ref().unwrap().selected, 1, "the selection stays on the list");
        app.update(press(KeyCode::Esc), at(1));
        assert!(app.composer.picker.is_none());
        assert_eq!(app.focus, Focus::Composer, "Esc closes the picker first");
    }

    #[test]
    fn a_picker_opened_while_typing_returns_to_the_task() {
        let mut app = composing();
        type_text(&mut app, "Fix ");
        app.update(press(KeyCode::F(3)), at(1));
        type_text(&mut app, "codex");
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.composer.field, Field::Task);
        type_text(&mut app, "login");
        assert_eq!(app.composer.task.text(), "Fix login", "typing goes on in the task");
        app.update(press(KeyCode::F(2)), at(1));
        app.update(press(KeyCode::Esc), at(1));
        assert_eq!(app.composer.field, Field::Task, "Esc returns too");
    }

    #[test]
    fn a_picker_opened_from_its_row_stays_on_the_row() {
        let mut app = composing();
        for _ in 0..4 {
            app.update(press(KeyCode::Tab), at(1));
        }
        assert_eq!(app.composer.field, Field::Harness, "task, project, machine, preset, harness");
        app.update(press(KeyCode::Enter), at(1));
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.composer.field, Field::Harness);
    }

    #[test]
    fn typing_on_a_field_row_opens_its_picker_with_that_letter() {
        let mut app = composing();
        app.update(press(KeyCode::Tab), at(1));
        assert_eq!(app.composer.field, Field::Project);
        app.update(press(KeyCode::Char('c')), at(1));
        let picker = app.composer.picker.as_ref().unwrap();
        assert_eq!((picker.field, picker.query.as_str()), (Field::Project, "c"));
    }

    #[test]
    fn down_from_the_last_task_line_moves_to_the_fields() {
        let mut app = composing();
        type_text(&mut app, "x");
        app.update(press(KeyCode::Down), at(1));
        assert_eq!(app.composer.field, Field::Project);
        app.update(press(KeyCode::Up), at(1));
        assert_eq!(app.composer.field, Field::Task);
    }

    #[test]
    fn paste_goes_into_the_task_or_the_pickers_query() {
        let mut app = composing();
        app.update(Input::Paste("line one\nline two".into()), at(1));
        assert_eq!(app.composer.task.text(), "line one\nline two");
        app.update(press(KeyCode::F(2)), at(1));
        app.update(Input::Paste("cock\npit".into()), at(1));
        assert_eq!(app.composer.picker.as_ref().unwrap().query, "cock");
    }

    fn sent(app: &mut App) -> Record {
        type_text(app, "Fix login");
        let effects = app.update(press(KeyCode::Enter), at(5));
        launch_of(&effects).unwrap().clone()
    }

    #[test]
    fn progress_and_success_update_the_launch_and_remember_the_choices() {
        let mut app = composing();
        let mut record = sent(&mut app);
        app.update(Input::LaunchProgress { id: record.id.clone(), text: "starting claude".into() }, at(6));
        assert_eq!(app.launches[0].state, LaunchState::Running("starting claude".into()));
        record.stage = Stage::Submitted;
        record.pane_id = Some("w9:p1".into());
        let effects =
            app.update(Input::LaunchFinished { id: record.id.clone(), result: Ok(Outcome::Sent(record)) }, at(7));
        assert_eq!(app.launches[0].state, LaunchState::Sent { unverified: false });
        assert!(
            matches!(&effects[..], [Effect::Remember(r)] if r.project == "cockpit" && r.harness == "claude" && r.workspace == "worktree")
        );
        assert_eq!(app.preferences.last_project.as_deref(), Some("cockpit"));
    }

    #[test]
    fn a_failed_launch_shows_its_first_error_line() {
        let mut app = composing();
        let record = sent(&mut app);
        let result = Err(launch::Failure {
            record: Box::new(record.clone()),
            error: "expected claude, detected bash\nmore detail".into(),
        });
        let effects = app.update(Input::LaunchFinished { id: record.id.clone(), result }, at(7));
        assert!(effects.is_empty(), "a failure is not remembered");
        assert_eq!(app.launches[0].state, LaunchState::Failed("expected claude, detected bash".into()));
        assert!(app.notice.as_ref().unwrap().text.contains("expected claude, detected bash"));
    }

    #[test]
    fn a_launch_waiting_at_a_startup_dialog_sends_its_task_once_the_agent_is_idle() {
        let mut app = composing();
        let mut record = sent(&mut app);
        record.stage = Stage::StartupBlocked;
        record.pane_id = Some("w2:p1".into());
        app.update(
            Input::LaunchFinished { id: record.id.clone(), result: Ok(Outcome::WaitingForStartup(record.clone())) },
            at(7),
        );
        assert_eq!(app.launches[0].state, LaunchState::Waiting, "w2:p1 is still working");
        assert!(app.waiting.contains_key(&l("w2:p1")));
        let effects = app.update(status("w2:p1", AgentStatus::Blocked), at(8));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Resume { .. })), "the dialog is still up");
        let effects = app.update(status("w2:p1", AgentStatus::Idle), at(9));
        assert!(
            matches!(&effects[..], [Effect::Resume { machine, record: r }] if machine == LOCAL && r.id == record.id)
        );
        assert!(app.waiting.is_empty());
        assert_eq!(app.launches[0].state, LaunchState::Running("sending the task".into()));
        app.update(Input::Resumed { result: Ok(Outcome::WaitingForStartup(record.clone())) }, at(10));
        assert_eq!(app.launches[0].state, LaunchState::Waiting, "a second dialog");
        assert!(app.waiting.contains_key(&l("w2:p1")));
        app.update(Input::Resumed { result: Ok(Outcome::Sent(record.clone())) }, at(11));
        assert_eq!(app.launches[0].state, LaunchState::Sent { unverified: false });
    }

    #[test]
    fn launches_from_an_earlier_run_resume_too() {
        let (mut app, _) = loaded();
        let record = Record {
            id: "old".into(),
            machine_id: LOCAL.into(),
            machine_label: "Local".into(),
            project: "cockpit".into(),
            repo: "/w/cockpit".into(),
            harness: "claude".into(),
            model: String::new(),
            thinking: String::new(),
            title: "Old task".into(),
            task: "Old task".into(),
            agent_name: "t-old".into(),
            created_at: 0,
            stage: Stage::StartupBlocked,
            workspace: "worktree".into(),
            branch: "old".into(),
            cwd: String::new(),
            workspace_id: Some("w3".into()),
            pane_id: Some("w3:p1".into()),
            tab_id: None,
            unverified: false,
            failed_stage: None,
            error: None,
        };
        let effects = app.update(Input::Journals(vec![record]), at(1));
        assert!(matches!(&effects[..], [Effect::Resume { .. }]), "w3:p1 is idle already");
    }

    #[test]
    fn a_failed_resume_says_so() {
        let mut app = composing();
        let record = sent(&mut app);
        app.update(
            Input::Resumed {
                result: Err(launch::Failure { record: Box::new(record.clone()), error: "agent is blocked".into() }),
            },
            at(9),
        );
        assert_eq!(app.launches[0].state, LaunchState::Failed("agent is blocked".into()));
        assert!(app.notice.as_ref().unwrap().kind == NoticeKind::Error);
    }

    #[test]
    fn the_launch_list_keeps_the_most_recent_eight() {
        let mut app = composing();
        for i in 0..10 {
            type_text(&mut app, &format!("task {i}"));
            app.update(press(KeyCode::Enter), at(5));
        }
        assert_eq!(app.launches.len(), 8);
        assert_eq!(app.launches[0].title, "task 2");
    }
}

mod conveniences {
    use super::*;
    use crate::discovery::{Catalog, CheckoutEntry, Choice as ModelChoice, Inventory, Project};
    use crate::launch::{Outcome, Record, Stage};
    use crate::presets::Preset;
    use crate::threads::LaunchNote;
    use std::collections::BTreeMap;

    fn inventory() -> Inventory {
        Inventory {
            projects: vec![Project {
                name: "cockpit".into(),
                path: "/w/cockpit".into(),
                branch: "main".into(),
                checkouts: vec![CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false }],
            }],
            harnesses: vec!["claude".into(), "codex".into()],
            models: BTreeMap::from([(
                "claude".into(),
                Catalog {
                    choices: vec![ModelChoice { id: "opus".into(), label: "Opus".into() }],
                    selectable: true,
                    thinking_flag: "--effort".into(),
                    thinking: vec!["high".into()],
                    ..Catalog::default()
                },
            )]),
            models_at: 0,
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
    }

    fn ctrl(app: &mut App, c: char) -> Vec<Effect> {
        app.update(press_with(KeyCode::Char(c), KeyModifiers::CONTROL), at(1))
    }

    fn composing(presets: Vec<Preset>, history: Vec<String>) -> App {
        let (app, _) = loaded();
        let mut app = app.with_memory(presets, history);
        app.update(press(KeyCode::Char('n')), at(1));
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory()) }, at(1));
        app
    }

    fn preset(name: &str, harness: &str, model: &str, thinking: &str) -> Preset {
        Preset { name: name.into(), harness: harness.into(), model: model.into(), thinking: thinking.into() }
    }

    fn record(id: &str, pane: Option<&str>, stage: Stage) -> Record {
        Record {
            id: id.into(),
            machine_id: LOCAL.into(),
            machine_label: "Local".into(),
            project: "cockpit".into(),
            repo: "/w/cockpit".into(),
            harness: "claude".into(),
            model: "opus".into(),
            thinking: "high".into(),
            title: format!("Task {id}"),
            task: format!("Task {id}\nwith details"),
            agent_name: format!("t-{id}"),
            created_at: 5,
            stage,
            workspace: "worktree".into(),
            branch: format!("branch-{id}"),
            cwd: String::new(),
            workspace_id: pane.map(|p| p.split(':').next().unwrap().to_string()),
            pane_id: pane.map(str::to_string),
            tab_id: None,
            unverified: false,
            failed_stage: None,
            error: Some("expected claude, detected bash\nfull log".into()),
        }
    }

    #[test]
    fn ctrl_d_saves_the_current_choices_as_a_preset() {
        let mut app = composing(vec![], vec![]);
        assert!(ctrl(&mut app, 'd').is_empty());
        assert_eq!(app.composer.error.as_deref(), Some("Pick a model first to save a preset."));
        app.update(press(KeyCode::F(4)), at(1));
        type_text(&mut app, "opus");
        app.update(press(KeyCode::Enter), at(1));
        ctrl(&mut app, 'd');
        let picker = app.composer.picker.as_ref().unwrap();
        assert_eq!((picker.field, picker.query.as_str()), (Field::Preset, "Claude · Opus"));
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(effects, vec![Effect::SavePresets(vec![preset("Claude · Opus", "claude", "opus", "")])]);
        assert_eq!(app.composer.value(&app.composer_context(), Field::Preset), "Claude · Opus");
    }

    #[test]
    fn picking_a_preset_sets_harness_model_and_thinking() {
        let mut app =
            composing(vec![preset("Deep", "claude", "opus", "high"), preset("Fast", "codex", "gpt-5", "")], vec![]);
        app.update(press(KeyCode::F(3)), at(1));
        type_text(&mut app, "codex");
        app.update(press(KeyCode::Enter), at(1));
        app.update(press(KeyCode::F(7)), at(1));
        type_text(&mut app, "deep");
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.composer.harness.as_deref(), Some("claude"));
        assert_eq!(app.composer.model.as_deref(), Some("opus"));
        assert_eq!(app.composer.thinking.as_deref(), Some("high"));
        assert_eq!(app.composer.matching_preset(&app.presets).map(|p| p.name.as_str()), Some("Deep"));
    }

    #[test]
    fn typing_finds_presets_before_offering_to_save_one() {
        let mut app = composing(vec![preset("Deep", "claude", "opus", "high")], vec![]);
        app.composer.model = Some("opus".into());
        app.update(press(KeyCode::F(7)), at(1));
        type_text(&mut app, "dee");
        let labels: Vec<String> = app
            .composer
            .choices_with_actions(&app.composer_context(), Field::Preset, "dee")
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(labels, ["Deep", "Save as dee"]);
    }

    #[test]
    fn a_preset_for_a_cli_missing_here_is_shown_but_not_pickable() {
        let mut app = composing(vec![preset("Pi", "pi", "x", "")], vec![]);
        app.update(press(KeyCode::F(7)), at(1));
        let choices = app.composer.choices_with_actions(&app.composer_context(), Field::Preset, "");
        assert_eq!(choices[0].detail, "not installed here");
        assert!(!choices[0].enabled);
        app.update(press(KeyCode::Enter), at(1));
        assert!(app.composer.picker.is_some(), "a disabled choice does nothing");
    }

    #[test]
    fn presets_are_deleted_and_renamed_inside_the_picker() {
        let mut app = composing(vec![preset("A", "claude", "opus", ""), preset("B", "codex", "gpt-5", "")], vec![]);
        app.update(press(KeyCode::F(7)), at(1));
        let effects = app.update(press(KeyCode::Delete), at(1));
        assert_eq!(effects, vec![Effect::SavePresets(vec![preset("B", "codex", "gpt-5", "")])]);
        ctrl(&mut app, 'r');
        assert_eq!(app.composer.picker.as_ref().unwrap().renaming.as_deref(), Some("B"));
        app.update(press(KeyCode::Backspace), at(1));
        type_text(&mut app, "Quick");
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(effects, vec![Effect::SavePresets(vec![preset("Quick", "codex", "gpt-5", "")])]);
    }

    #[test]
    fn renaming_onto_a_taken_name_is_refused() {
        let mut app = composing(vec![preset("A", "claude", "opus", ""), preset("B", "codex", "gpt-5", "")], vec![]);
        app.update(press(KeyCode::F(7)), at(1));
        ctrl(&mut app, 'r');
        for _ in 0..1 {
            app.update(press(KeyCode::Backspace), at(1));
        }
        type_text(&mut app, "B");
        assert!(app.update(press(KeyCode::Enter), at(1)).is_empty());
        assert!(app.composer.error.as_deref().unwrap().contains("already has this name"));
    }

    #[test]
    fn ctrl_p_and_ctrl_n_browse_the_history_and_restore_the_draft() {
        let mut app = composing(vec![], vec!["newest".into(), "older".into()]);
        type_text(&mut app, "draft");
        ctrl(&mut app, 'p');
        assert_eq!(app.composer.task.text(), "newest");
        ctrl(&mut app, 'p');
        ctrl(&mut app, 'p');
        assert_eq!(app.composer.task.text(), "older", "stops at the oldest");
        ctrl(&mut app, 'n');
        ctrl(&mut app, 'n');
        assert_eq!(app.composer.task.text(), "draft");
        ctrl(&mut app, 'n');
        assert_eq!(app.composer.task.text(), "draft", "nothing newer than the draft");
    }

    #[test]
    fn sending_puts_the_task_on_top_of_the_history() {
        let mut app = composing(vec![], vec!["Fix login".into(), "older".into()]);
        type_text(&mut app, "Fix login");
        app.update(press(KeyCode::Enter), at(5));
        assert_eq!(app.history, ["Fix login", "older"]);
    }

    #[test]
    fn slash_filters_the_list_as_you_type() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('/')), at(1));
        assert!(app.filtering);
        type_text(&mut app, "bld");
        assert_eq!(ids(&app), ["w2:p1"], "fuzzy: Build");
        assert_eq!(app.cursor.as_deref(), Some(l("w2:p1").as_str()), "the cursor stays on a visible thread");
        app.update(press(KeyCode::Enter), at(1));
        assert!(!app.filtering);
        assert_eq!(ids(&app), ["w2:p1"], "Enter keeps the filter");
        assert!(app.update(press(KeyCode::Char('j')), at(1)).is_empty(), "j moves again");
        app.update(press(KeyCode::Esc), at(1));
        assert_eq!(ids(&app).len(), 3, "Esc clears it");
    }

    #[test]
    fn the_filter_matches_one_field_at_a_time() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('/')), at(1));
        type_text(&mut app, "bdcl");
        assert!(app.threads.is_empty(), "Build + Docs + Claude letters do not add up across fields");
    }

    #[test]
    fn the_filter_survives_new_snapshots() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('/')), at(1));
        type_text(&mut app, "docs");
        app.update(
            snapshot(vec![agent("w3:p1", AgentStatus::Idle, "Docs"), agent("w4:p1", AgentStatus::Idle, "Other")]),
            at(2),
        );
        assert_eq!(ids(&app), ["w3:p1"]);
    }

    #[test]
    fn backspace_on_an_empty_filter_leaves_filter_mode() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('/')), at(1));
        app.update(press(KeyCode::Backspace), at(1));
        assert!(!app.filtering);
    }

    #[test]
    fn r_replies_to_an_agent_without_opening_it() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('r')), at(1));
        assert!(app.reply.is_some());
        type_text(&mut app, "also add tests");
        app.update(press_with(KeyCode::Enter, KeyModifiers::SHIFT), at(1));
        type_text(&mut app, "thanks");
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(
            effects,
            vec![Effect::Prompt {
                machine: LOCAL.into(),
                pane_id: "w3:p1".into(),
                title: "Docs".into(),
                text: "also add tests\nthanks".into(),
            }]
        );
        assert!(app.reply.is_none());
        assert_eq!(app.focus, Focus::List, "replying never leaves the list");
    }

    #[test]
    fn a_blocked_agent_cannot_be_replied_to_from_the_list() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('r')), at(1));
        assert!(app.reply.is_none());
        assert!(app.notice.as_ref().unwrap().text.contains("is waiting for an answer"));
    }

    #[test]
    fn an_empty_reply_is_not_sent_and_esc_cancels() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('r')), at(1));
        assert!(app.update(press(KeyCode::Enter), at(1)).is_empty());
        app.update(press(KeyCode::Esc), at(1));
        assert!(app.reply.is_none());
    }

    #[test]
    fn reply_results_are_reported() {
        let (mut app, _) = loaded();
        let prompted = |result| Input::Prompted { title: "Docs".into(), text: "add tests".into(), result };
        app.update(prompted(Ok(false)), at(1));
        assert_eq!(app.notice.as_ref().unwrap().text, "Sent to “Docs”");
        app.update(prompted(Ok(true)), at(1));
        assert!(app.notice.as_ref().unwrap().text.contains("not confirmed"));
        app.update(prompted(Err("agent Docs is blocked".into())), at(1));
        assert!(app.notice.as_ref().unwrap().text.contains("waiting for an answer"));
        assert!(app.notice.as_ref().unwrap().text.ends_with("Your words: “add tests”"), "the words are never lost");
        app.update(prompted(Err("pane not found".into())), at(1));
        assert!(app.notice.as_ref().unwrap().text.contains("pane not found"));
    }

    #[test]
    fn e_brings_a_launchs_images_back_as_placeholders() {
        let (mut app, _) = loaded();
        let mut launch = record("a1", Some("w2:p1"), Stage::Submitted);
        launch.task = "fix /h/.cache/herdr-inbox/images/image-9.png now".into();
        app.update(Input::Journals(vec![launch]), at(1));
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('e')), at(1));
        assert_eq!(app.composer.task.text(), "fix [Image #1] now");
        assert_eq!(app.composer.images, ["/h/.cache/herdr-inbox/images/image-9.png"]);
    }

    #[test]
    fn e_fills_the_composer_from_the_threads_launch() {
        let (mut app, _) = loaded();
        app.update(Input::Journals(vec![record("a1", Some("w2:p1"), Stage::Submitted)]), at(1));
        app.update(press(KeyCode::Char('j')), at(1));
        let effects = app.update(press(KeyCode::Char('e')), at(1));
        assert_eq!(app.focus, Focus::Composer);
        assert!(effects.iter().any(|e| matches!(e, Effect::Discover { .. })));
        assert_eq!(app.composer.task.text(), "Task a1\nwith details");
        assert_eq!(app.composer.project.as_deref(), Some("cockpit"));
        assert_eq!(app.composer.harness.as_deref(), Some("claude"));
        assert_eq!(app.composer.model.as_deref(), Some("opus"));
        assert_eq!(app.composer.thinking.as_deref(), Some("high"));
        assert_eq!(app.composer.workspace, WorkspaceSel::New, "a delivered launch gets a fresh worktree");
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory()) }, at(2));
        assert_eq!(app.composer.model.as_deref(), Some("opus"), "the choices survive discovery");
    }

    #[test]
    fn e_on_a_thread_without_a_launch_keeps_its_project_and_harness() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('e')), at(1));
        assert_eq!(app.composer.task.text(), "");
        assert_eq!(app.composer.harness.as_deref(), Some("claude"));
        assert_eq!(app.composer.machine.as_deref(), Some(LOCAL));
        assert_eq!(app.composer.workspace, WorkspaceSel::New);
    }

    #[test]
    fn a_failed_launch_becomes_a_row_that_can_be_retried_or_dismissed() {
        let (mut app, _) = loaded();
        app.update(Input::Journals(vec![record("f1", None, Stage::NeedsAttention)]), at(1));
        let failed = app.thread("launch/f1").expect("a row for the failed launch").clone();
        assert_eq!(failed.status, AgentStatus::Blocked);
        assert_eq!(failed.note, Some(LaunchNote::Failed("expected claude, detected bash".into())));
        assert_eq!(failed.title, "Task f1");
        assert_eq!(app.threads[0].id, "launch/f1", "the newest needs-input row comes first");
        app.cursor = Some("launch/f1".into());
        assert!(app.update(press(KeyCode::Enter), at(1)).is_empty(), "no pane to open");
        assert!(app.notice.as_ref().unwrap().text.contains("never got a pane"));
        assert_eq!(app.focus, Focus::List, "the keyboard stays on the list");
        app.update(press(KeyCode::Char('e')), at(1));
        assert_eq!(app.composer.workspace, WorkspaceSel::Named("branch-f1".into()), "retrying keeps the branch");
        app.update(press(KeyCode::Esc), at(1));
        app.focus = Focus::List;
        app.cursor = Some("launch/f1".into());
        let effects = app.update(press(KeyCode::Char('d')), at(1));
        assert_eq!(effects, vec![Effect::Dismiss { record: "f1".into() }]);
        assert!(app.thread("launch/f1").is_none());
        assert!(app.cursor.is_some());
    }

    #[test]
    fn archiving_a_failed_launch_closes_its_workspace_and_forgets_it() {
        let (mut app, _) = loaded();
        app.update(Input::Journals(vec![record("f2", Some("w9:p1"), Stage::NeedsAttention)]), at(1));
        app.cursor = Some("launch/f2".into());
        app.update(press(KeyCode::Char('x')), at(1));
        let effects = app.update(press(KeyCode::Char('y')), at(1));
        assert!(effects.iter().any(|e| matches!(e, Effect::Archive { workspace_id, .. } if workspace_id == "w9")));
        assert!(effects.contains(&Effect::Dismiss { record: "f2".into() }));
    }

    #[test]
    fn d_only_dismisses_failed_launches() {
        let (mut app, _) = loaded();
        assert!(app.update(press(KeyCode::Char('d')), at(1)).is_empty());
        assert_eq!(app.threads.len(), 3);
    }

    #[test]
    fn launch_notes_show_on_live_threads() {
        let (mut app, _) = loaded();
        let mut unverified = record("u1", Some("w3:p1"), Stage::Submitted);
        unverified.unverified = true;
        app.update(
            Input::Journals(vec![
                record("b1", Some("w1:p1"), Stage::StartupBlocked),
                unverified,
                record("ok", Some("w2:p1"), Stage::Submitted),
            ]),
            at(1),
        );
        assert_eq!(app.thread(&l("w1:p1")).unwrap().note, Some(LaunchNote::Waiting));
        assert_eq!(app.thread(&l("w3:p1")).unwrap().note, Some(LaunchNote::Unverified));
        assert_eq!(app.thread(&l("w2:p1")).unwrap().note, None);
        assert!(app.waiting.contains_key(&l("w1:p1")), "a waiting journal resumes too");
    }

    #[test]
    fn a_failure_reported_live_also_becomes_a_row() {
        let mut app = composing(vec![], vec![]);
        type_text(&mut app, "Fix login");
        let effects = app.update(press(KeyCode::Enter), at(5));
        let Some(Effect::Launch { plan, .. }) = effects.iter().find(|e| matches!(e, Effect::Launch { .. })) else {
            panic!("no launch");
        };
        let mut failed = plan.record.clone();
        failed.stage = Stage::NeedsAttention;
        failed.error = Some("boom".into());
        app.update(
            Input::LaunchFinished {
                id: failed.id.clone(),
                result: Err(crate::launch::Failure { record: Box::new(failed.clone()), error: "boom".into() }),
            },
            at(6),
        );
        assert!(app.thread(&format!("launch/{}", failed.id)).is_some());
        let _ = Outcome::Sent(failed);
    }
}

mod voice {
    use super::*;
    use crate::app::{Dictation, Entry, Menu, Phase, SpeechStatus, Target};
    use crate::speech::backends::Credentials;

    fn ready(mut app: App) -> App {
        app.update(Input::Speech(SpeechStatus { ready: Some("Groq Whisper".into()), tools: vec![] }), at(0));
        app
    }

    fn ctrl_t(app: &mut App) -> Vec<Effect> {
        app.update(press_with(KeyCode::Char('t'), KeyModifiers::CONTROL), at(1))
    }

    fn recording(app: &mut App) {
        app.update(Input::DictationStarted(Ok(())), at(1));
        assert_eq!(app.dictation.as_ref().map(|d| d.phase), Some(Phase::Recording));
    }

    #[test]
    fn ctrl_t_without_a_transcriber_opens_the_menu_instead_of_recording() {
        let (mut app, _) = loaded();
        assert!(ctrl_t(&mut app).is_empty());
        assert!(app.dictation.is_none());
        let menu = app.menu.as_ref().expect("the menu opens");
        assert_eq!(menu.status.as_ref().map(|(s, _)| s.as_str()), Some("Connect a transcription service to dictate."));
    }

    #[test]
    fn ctrl_t_targets_what_has_the_keyboard() {
        let (app, _) = loaded();
        let mut app = ready(app);
        assert_eq!(ctrl_t(&mut app), vec![Effect::StartDictation]);
        assert_eq!(
            app.dictation.as_ref().unwrap().target,
            Target::Thread(l("w1:p1")),
            "the cursor's thread from the list"
        );
        app.update(press(KeyCode::Esc), at(1));
        app.update(press(KeyCode::Enter), at(1));
        ctrl_t(&mut app);
        assert_eq!(
            app.dictation.as_ref().unwrap().target,
            Target::Thread(l("w1:p1")),
            "the open agent from the terminal"
        );
        app.update(press(KeyCode::Esc), at(1));
        app.update(press(KeyCode::Tab), at(1));
        app.update(press(KeyCode::Char('n')), at(1));
        ctrl_t(&mut app);
        assert_eq!(app.dictation.as_ref().unwrap().target, Target::Composer);
    }

    #[test]
    fn a_failed_launch_row_cannot_be_dictated_to() {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.cursor = Some("launch/x".into());
        assert!(ctrl_t(&mut app).is_empty());
        assert!(app.dictation.is_none());
    }

    #[test]
    fn while_recording_no_key_reaches_the_agent() {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.update(press(KeyCode::Enter), at(1));
        ctrl_t(&mut app);
        recording(&mut app);
        for code in [KeyCode::Char('x'), KeyCode::Tab, KeyCode::Char('q'), KeyCode::Up] {
            assert!(app.update(press(code), at(1)).is_empty(), "{code:?} must not leak");
        }
        assert_eq!(app.focus, Focus::Terminal);
    }

    #[test]
    fn enter_sends_and_ctrl_t_types_into_the_agent() {
        let (app, _) = loaded();
        let mut app = ready(app);
        ctrl_t(&mut app);
        recording(&mut app);
        assert_eq!(app.update(press(KeyCode::Enter), at(2)), vec![Effect::StopDictation]);
        assert_eq!(app.dictation.as_ref().unwrap().phase, Phase::Transcribing);
        let effects = app.update(Input::Transcribed(Ok("fix the tests".into())), at(3));
        assert_eq!(
            effects,
            vec![Effect::Prompt {
                machine: LOCAL.into(),
                pane_id: "w1:p1".into(),
                title: "Login".into(),
                text: "fix the tests".into()
            }]
        );
        assert!(app.dictation.is_none());
        ctrl_t(&mut app);
        recording(&mut app);
        assert_eq!(ctrl_t(&mut app), vec![Effect::StopDictation]);
        let effects = app.update(Input::Transcribed(Ok("draft words".into())), at(4));
        assert_eq!(
            effects,
            vec![Effect::TypeText {
                machine: LOCAL.into(),
                pane_id: "w1:p1".into(),
                title: "Login".into(),
                text: "draft words".into()
            }]
        );
    }

    #[test]
    fn dictating_into_the_composer_inserts_or_sends() {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.update(press(KeyCode::Char('n')), at(1));
        for c in "Fix".chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
        ctrl_t(&mut app);
        recording(&mut app);
        ctrl_t(&mut app);
        app.update(Input::Transcribed(Ok("the login".into())), at(2));
        assert_eq!(app.composer.task.text(), "Fix the login", "a space joins the words");
        ctrl_t(&mut app);
        recording(&mut app);
        app.update(press(KeyCode::Enter), at(2));
        let effects = app.update(Input::Transcribed(Ok("loop".into())), at(3));
        assert_eq!(app.composer.error.as_deref(), Some("Pick a project."), "sending ran (and needs a project here)");
        assert!(effects.is_empty());
        assert_eq!(app.composer.task.text(), "Fix the login loop");
    }

    #[test]
    fn dictating_into_a_reply_fills_it_then_sends() {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('r')), at(1));
        ctrl_t(&mut app);
        assert_eq!(app.dictation.as_ref().unwrap().target, Target::Reply);
        recording(&mut app);
        app.update(press(KeyCode::Enter), at(2));
        let effects = app.update(Input::Transcribed(Ok("also docs".into())), at(3));
        assert_eq!(
            effects,
            vec![Effect::Prompt {
                machine: LOCAL.into(),
                pane_id: "w3:p1".into(),
                title: "Docs".into(),
                text: "also docs".into()
            }]
        );
        assert!(app.reply.is_none());
    }

    #[test]
    fn esc_discards_even_a_transcript_that_arrives_late() {
        let (app, _) = loaded();
        let mut app = ready(app);
        ctrl_t(&mut app);
        recording(&mut app);
        app.update(press(KeyCode::Enter), at(2));
        assert_eq!(app.update(press(KeyCode::Esc), at(2)), vec![Effect::CancelDictation]);
        assert!(app.update(Input::Transcribed(Ok("too late".into())), at(3)).is_empty());
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_recorder_or_transcription_failure_is_reported() {
        let (app, _) = loaded();
        let mut app = ready(app);
        ctrl_t(&mut app);
        app.update(Input::DictationStarted(Err("No microphone recorder found.".into())), at(1));
        assert!(app.dictation.is_none());
        assert_eq!(app.notice.as_ref().unwrap().text, "No microphone recorder found.");
        ctrl_t(&mut app);
        recording(&mut app);
        app.update(press(KeyCode::Enter), at(2));
        app.update(Input::Transcribed(Err("Transcription failed: HTTP 401: bad key".into())), at(3));
        assert!(app.dictation.is_none());
        assert!(app.notice.as_ref().unwrap().text.contains("HTTP 401"));
    }

    #[test]
    fn levels_feed_the_meter() {
        let (app, _) = loaded();
        let mut app = ready(app);
        ctrl_t(&mut app);
        recording(&mut app);
        app.update(Input::Levels { levels: vec![0.5; 12], quiet: true }, at(2));
        let d: &Dictation = app.dictation.as_ref().unwrap();
        assert_eq!(d.levels, vec![0.5; 12]);
        assert!(d.quiet);
    }

    #[test]
    fn the_menu_connects_a_service_after_verifying_its_key() {
        let (mut app, _) = loaded();
        assert!(app.update(press(KeyCode::F(10)), at(1)).is_empty());
        assert_eq!(app.menu, Some(Menu::default()));
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(effects, vec![Effect::OpenUrl("https://console.groq.com/keys".into())], "the key page opens");
        assert!(matches!(app.menu.as_ref().unwrap().entry, Some(Entry::Key { service: "groq", .. })));
        for c in "gsk_123".chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(effects, vec![Effect::VerifyKey { service: "groq", key: "gsk_123".into() }]);
        let effects = app.update(Input::KeyVerified { service: "groq", key: "gsk_123".into(), result: Ok(()) }, at(2));
        let Some(Effect::SaveCredentials(saved)) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(saved.keys["groq"], "gsk_123");
        assert_eq!(saved.backend, "groq");
        assert!(app.menu.as_ref().unwrap().status.as_ref().unwrap().0.contains("Groq Whisper connected"));
    }

    #[test]
    fn a_rejected_key_is_shown_and_not_saved() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::F(10)), at(1));
        let effects = app.update(
            Input::KeyVerified {
                service: "groq",
                key: "bad".into(),
                result: Err("Groq Whisper rejected the key: HTTP 401".into()),
            },
            at(2),
        );
        assert!(effects.is_empty());
        assert_eq!(app.menu.as_ref().unwrap().status, Some(("Groq Whisper rejected the key: HTTP 401".into(), true)));
        assert!(app.credentials.keys.is_empty());
    }

    #[test]
    fn the_menu_saves_a_custom_command_and_disconnects() {
        let (app, _) = loaded();
        let mut app = app.with_credentials(Credentials {
            backend: String::new(),
            keys: [("groq".into(), "k".into())].into(),
            command: String::new(),
        });
        app.update(press(KeyCode::F(10)), at(1));
        for _ in 0..6 {
            app.update(press(KeyCode::Down), at(1));
        }
        app.update(press(KeyCode::Enter), at(1));
        assert!(matches!(app.menu.as_ref().unwrap().entry, Some(Entry::Command { .. })));
        for c in "stt {file}".chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
        let effects = app.update(press(KeyCode::Enter), at(1));
        let Some(Effect::SaveCredentials(saved)) = effects.first() else { panic!("{effects:?}") };
        assert_eq!((saved.backend.as_str(), saved.command.as_str()), ("command", "stt {file}"));
        assert_eq!(saved.keys["groq"], "k", "other keys are kept");
        app.update(press(KeyCode::Down), at(1));
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(effects, vec![Effect::SaveCredentials(Credentials::default())]);
        assert_eq!(app.update(press(KeyCode::Esc), at(1)), vec![]);
        assert!(app.menu.is_none());
    }

    #[test]
    fn building_whisper_reports_progress_then_saves_the_command() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::F(10)), at(1));
        for _ in 0..5 {
            app.update(press(KeyCode::Down), at(1));
        }
        assert_eq!(app.update(press(KeyCode::Enter), at(1)), vec![Effect::InstallWhisper]);
        app.update(Input::Whisper { result: Ok(None), progress: Some("Compiling whisper.cpp".into()) }, at(2));
        assert_eq!(app.menu.as_ref().unwrap().status, Some(("Compiling whisper.cpp".into(), false)));
        let effects =
            app.update(Input::Whisper { result: Ok(Some("/w/whisper-cli -f {file}".into())), progress: None }, at(3));
        assert!(matches!(&effects[..], [Effect::SaveCredentials(c)] if c.command == "/w/whisper-cli -f {file}"));
        app.update(
            Input::Whisper { result: Err("Local whisper needs cmake installed.".into()), progress: None },
            at(4),
        );
        assert_eq!(app.menu.as_ref().unwrap().status, Some(("Local whisper needs cmake installed.".into(), true)));
    }

    #[test]
    fn the_menu_swallows_keys_meant_for_threads() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::F(10)), at(1));
        assert!(app.update(press(KeyCode::Char('q')), at(1)).is_empty(), "q does not quit");
        assert!(app.menu.is_some());
    }
}

mod space_bar {
    use super::*;
    use crate::app::{Phase, SpeechStatus, Target, Then};
    use crate::speech::backends::SpeechConfig;

    /// Milliseconds into the session.
    fn ms(ms: u64) -> SystemTime {
        at(100) + Duration::from_millis(ms)
    }

    fn ready(mut app: App) -> App {
        app.update(Input::Speech(SpeechStatus { ready: Some("Groq Whisper".into()), tools: vec![] }), at(0));
        app
    }

    /// The loaded app with its first thread open and focused.
    fn in_agent() -> App {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.update(press(KeyCode::Enter), at(1));
        assert_eq!(app.focus, Focus::Terminal);
        app
    }

    fn space(app: &mut App, at_ms: u64) -> Vec<Effect> {
        app.update(press(KeyCode::Char(' ')), ms(at_ms))
    }

    fn tick(app: &mut App, at_ms: u64) -> Vec<Effect> {
        app.update(Input::Tick, ms(at_ms))
    }

    fn typed(bytes: &[u8]) -> Effect {
        Effect::Send { generation: 1, control: Control::Input(bytes.to_vec()) }
    }

    /// Holds the bar from `start` the way this keyboard repeats it, through
    /// `until`; returns every effect along the way.
    fn hold(app: &mut App, start: u64, until: u64) -> Vec<Effect> {
        let mut effects = space(app, start);
        let mut t = start + 250;
        while t <= until {
            effects.extend(space(app, t));
            t += 25;
        }
        effects
    }

    #[test]
    fn a_typed_space_reaches_the_agent_with_the_next_key_in_order() {
        let mut app = in_agent();
        assert_eq!(app.update(press(KeyCode::Char('a')), ms(0)), vec![typed(b"a")]);
        assert!(space(&mut app, 90).is_empty(), "a space waits");
        assert_eq!(app.update(press(KeyCode::Char('b')), ms(160)), vec![typed(b" "), typed(b"b")]);
        assert!(space(&mut app, 300).is_empty());
        assert!(space(&mut app, 420).is_empty());
        assert_eq!(app.update(press(KeyCode::Enter), ms(500)), vec![typed(b" "), typed(b" "), typed(b"\r")]);
        assert!(!app.hold.busy());
    }

    #[test]
    fn a_space_with_nothing_after_it_arrives_once_it_cannot_be_a_hold() {
        let mut app = in_agent();
        space(&mut app, 0);
        assert!(tick(&mut app, 500).is_empty());
        assert_eq!(tick(&mut app, 701), vec![typed(b" ")]);
        assert!(tick(&mut app, 2_000).is_empty());
    }

    #[test]
    fn holding_space_on_an_agent_records_and_letting_go_types_the_words() {
        let mut app = in_agent();
        let effects = hold(&mut app, 0, 300);
        assert_eq!(effects, vec![Effect::StartDictation], "no space ever reaches the agent");
        let dictation = app.dictation.as_ref().expect("recording");
        assert!(dictation.held);
        assert_eq!(dictation.target, Target::Thread(l("w1:p1")));
        app.update(Input::DictationStarted(Ok(())), ms(320));
        // Keep holding for two seconds: the repeats are swallowed.
        let mut t = 325;
        while t <= 2_300 {
            assert!(space(&mut app, t).is_empty(), "at {t}");
            assert!(tick(&mut app, t + 10).is_empty(), "still held at {t}");
            t += 25;
        }
        // Let go.
        assert!(tick(&mut app, 2_440).is_empty(), "a repeat 140 ms late is not a release");
        assert_eq!(tick(&mut app, 2_460), vec![Effect::StopDictation]);
        let dictation = app.dictation.as_ref().unwrap();
        assert_eq!((dictation.phase, dictation.then), (Phase::Transcribing, Some(Then::Type)));
        let effects = app.update(Input::Transcribed(Ok("fix the tests".into())), ms(3_000));
        assert_eq!(
            effects,
            vec![Effect::TypeText {
                machine: LOCAL.into(),
                pane_id: "w1:p1".into(),
                title: "Login".into(),
                text: "fix the tests".into()
            }]
        );
        assert!(!app.hold.busy());
    }

    #[test]
    fn a_brief_hold_is_a_space_not_a_recording() {
        let mut app = in_agent();
        hold(&mut app, 0, 400);
        assert!(app.dictation.is_some());
        // Let go, noticed 260 ms after recording started: a long press on
        // space.
        assert_eq!(tick(&mut app, 560), vec![Effect::CancelDictation, typed(b" ")]);
        assert!(app.dictation.is_none());
    }

    #[test]
    fn enter_while_holding_sends_and_letting_go_after_does_nothing_more() {
        let mut app = in_agent();
        hold(&mut app, 0, 1_000);
        assert_eq!(app.update(press(KeyCode::Enter), ms(1_010)), vec![Effect::StopDictation]);
        assert_eq!(app.dictation.as_ref().unwrap().then, Some(Then::Send));
        hold_on(&mut app, 1_025, 1_200);
        assert!(tick(&mut app, 1_400).is_empty(), "released, already sending");
        assert!(!app.hold.busy());
    }

    fn hold_on(app: &mut App, from: u64, to: u64) {
        let mut t = from;
        while t <= to {
            assert!(space(app, t).is_empty());
            t += 25;
        }
    }

    #[test]
    fn esc_while_holding_discards_and_the_rest_of_the_hold_types_nothing() {
        let mut app = in_agent();
        hold(&mut app, 0, 1_000);
        assert_eq!(app.update(press(KeyCode::Esc), ms(1_010)), vec![Effect::CancelDictation]);
        hold_on(&mut app, 1_025, 1_500);
        assert!(tick(&mut app, 1_700).is_empty());
        assert!(app.dictation.is_none());
    }

    #[test]
    fn in_the_composer_spaces_type_and_a_hold_dictates_the_task() {
        let (app, _) = loaded();
        let mut app = ready(app);
        app.update(press(KeyCode::Char('n')), ms(0));
        for (i, c) in "fix it".chars().enumerate() {
            app.update(press(KeyCode::Char(c)), ms(10 + i as u64 * 120));
        }
        assert_eq!(app.composer.task.text(), "fix it");
        assert_eq!(hold(&mut app, 1_000, 1_300), vec![Effect::StartDictation]);
        assert_eq!(app.dictation.as_ref().unwrap().target, Target::Composer);
        assert_eq!(app.composer.task.text(), "fix it", "the held space is not in the task");
        app.update(Input::DictationStarted(Ok(())), ms(1_310));
        hold_on(&mut app, 1_325, 2_500);
        assert_eq!(tick(&mut app, 2_660), vec![Effect::StopDictation]);
        app.update(Input::Transcribed(Ok("the login loop".into())), ms(3_000));
        assert_eq!(app.composer.task.text(), "fix it the login loop", "typed, not sent");
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn in_the_list_a_hold_dictates_to_the_thread_under_the_cursor() {
        let (app, _) = loaded();
        let mut app = ready(app);
        assert_eq!(app.focus, Focus::List);
        app.update(press(KeyCode::Char('j')), ms(0));
        let cursor = app.cursor.clone().unwrap();
        assert_eq!(hold(&mut app, 100, 400), vec![Effect::StartDictation]);
        assert_eq!(app.dictation.as_ref().unwrap().target, Target::Thread(cursor));
    }

    #[test]
    fn where_space_means_something_else_it_never_waits() {
        let (app, _) = loaded();
        let mut app = ready(app);
        // The list filter types its spaces at once.
        app.update(press(KeyCode::Char('/')), ms(0));
        app.update(press(KeyCode::Char('a')), ms(10));
        space(&mut app, 20);
        assert_eq!(app.filter, "a ");
        assert!(!app.hold.busy());
        app.update(press(KeyCode::Esc), ms(30));
        // On a composer field, space opens its picker.
        app.update(press(KeyCode::Char('n')), ms(40));
        app.update(press(KeyCode::Tab), ms(50));
        assert_ne!(app.composer.field, crate::app::Field::Task);
        space(&mut app, 60);
        assert!(app.composer.picker.is_some(), "the picker opened at once");
        assert!(!app.hold.busy());
    }

    #[test]
    fn shift_space_and_a_disabled_hold_type_at_once() {
        let mut app = in_agent();
        assert_eq!(app.update(press_with(KeyCode::Char(' '), KeyModifiers::SHIFT), ms(0)).len(), 1);
        assert!(!app.hold.busy());
        app.config.speech = SpeechConfig { space_hold: Some(false), ..SpeechConfig::default() };
        assert_eq!(space(&mut app, 10), vec![typed(b" ")]);
        let effects = hold(&mut app, 100, 400);
        assert_eq!(effects, vec![typed(b" "); 4], "the press and three repeats, each a space");
        assert!(app.dictation.is_none());
    }

    #[test]
    fn without_a_transcriber_a_hold_opens_the_menu_and_types_nowhere() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Enter), at(1));
        assert!(hold(&mut app, 0, 300).is_empty());
        let menu = app.menu.as_ref().expect("the dictation menu");
        assert!(menu.entry.is_none());
        // The repeats neither move the menu nor reach the agent.
        let before = app.menu.clone();
        hold_on(&mut app, 325, 800);
        assert_eq!(app.menu, before);
        assert!(tick(&mut app, 1_000).is_empty());
    }

    #[test]
    fn ctrl_t_dictation_is_unchanged_and_spaces_while_recording_go_nowhere() {
        let mut app = in_agent();
        assert_eq!(
            app.update(press_with(KeyCode::Char('t'), KeyModifiers::CONTROL), ms(0)),
            vec![Effect::StartDictation]
        );
        assert!(!app.dictation.as_ref().unwrap().held);
        assert!(space(&mut app, 100).is_empty());
        assert!(!app.hold.busy(), "spaces while recording are not held");
        assert!(tick(&mut app, 2_000).is_empty());
    }

    #[test]
    fn a_paste_or_a_click_types_waiting_spaces_first_but_moving_the_mouse_does_not() {
        let mut app = in_agent();
        space(&mut app, 0);
        let effects = app.update(Input::Paste("x".into()), ms(10));
        assert_eq!(effects.first(), Some(&typed(b" ")));
        assert_eq!(effects.len(), 2);
        space(&mut app, 20);
        app.update(mouse(MouseEventKind::Moved, 50, 10), ms(30));
        assert!(app.hold.busy(), "moving the mouse leaves the space waiting");
        let effects = app.update(mouse(MouseEventKind::Down(MouseButton::Left), 2, 10), ms(40));
        assert_eq!(effects.first(), Some(&typed(b" ")), "a click types it first");
        assert!(!app.hold.busy());
    }

    #[test]
    fn ticks_come_fast_only_while_a_space_waits_or_the_bar_is_held() {
        let mut app = in_agent();
        assert_eq!(app.tick_every(), Duration::from_millis(500));
        space(&mut app, 0);
        assert_eq!(app.tick_every(), Duration::from_millis(20));
        tick(&mut app, 1_000);
        assert_eq!(app.tick_every(), Duration::from_millis(500));
        hold(&mut app, 2_000, 2_300);
        assert_eq!(app.tick_every(), Duration::from_millis(20), "watching for the release");
    }

    #[test]
    fn the_space_hold_setting_reads_from_the_speech_section() {
        let config: crate::config::Config = toml::from_str("[speech]\nspace_hold = false\n").unwrap();
        assert_eq!(config.speech.space_hold, Some(false));
        let config: crate::config::Config = toml::from_str("[speech]\nlanguage = \"fr\"\n").unwrap();
        assert_eq!(config.speech.space_hold, None, "on by default");
    }
}

mod pasted_images {
    use super::*;
    use crate::clipboard::Clip;
    use crate::discovery::{CheckoutEntry, Inventory, Project};
    use std::collections::BTreeMap;

    const A: &str = "/home/u/.cache/herdr-inbox/images/image-1.png";
    const B: &str = "/home/u/.cache/herdr-inbox/images/image-2.png";

    fn composing(history: Vec<String>) -> App {
        let (app, _) = loaded();
        let mut app = app.with_memory(Vec::new(), history);
        app.update(press(KeyCode::Char('n')), at(1));
        let inventory = Inventory {
            projects: vec![Project {
                name: "cockpit".into(),
                path: "/w/cockpit".into(),
                branch: "main".into(),
                checkouts: vec![CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false }],
            }],
            harnesses: vec!["claude".into()],
            models: BTreeMap::new(),
            models_at: 0,
        };
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory) }, at(1));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update(press(KeyCode::Char(c)), at(1));
        }
    }

    fn image(path: &str) -> Input {
        Input::Clipboard(Ok(Clip::Image(path.into())))
    }

    fn launched_task(effects: &[Effect]) -> String {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Launch { plan, .. } => Some(plan.record.task.clone()),
                _ => None,
            })
            .expect("a launch")
    }

    #[test]
    fn ctrl_v_and_the_desktop_paste_read_the_clipboard_from_the_task() {
        let mut app = composing(Vec::new());
        type_text(&mut app, "hi");
        for key in [
            press_with(KeyCode::Char('v'), KeyModifiers::CONTROL),
            // Ghostty hands Ctrl+Shift+V over as a key when only an image is copied.
            press_with(KeyCode::Char('V'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        ] {
            assert_eq!(app.update(key, at(1)), vec![Effect::ReadClipboard]);
        }
        assert_eq!(app.composer.task.text(), "hi", "nothing is typed while the clipboard is read");
    }

    #[test]
    fn a_pasted_image_shows_as_a_placeholder_and_is_sent_as_its_path() {
        let mut app = composing(Vec::new());
        type_text(&mut app, "why");
        app.update(image(A), at(2));
        assert_eq!(app.composer.task.text(), "why [Image #1] ");
        type_text(&mut app, "and");
        app.update(image(B), at(2));
        type_text(&mut app, "broken");
        assert_eq!(app.composer.task.text(), "why [Image #1] and [Image #2] broken");
        let effects = app.update(press(KeyCode::Enter), at(3));
        let task = format!("why {A} and {B} broken");
        assert_eq!(launched_task(&effects), task);
        assert!(effects.contains(&Effect::SaveHistory(task)), "the history keeps the images");
        assert_eq!(app.launches[0].title, "why and broken");
        assert_eq!(app.composer.task.text(), "");
        assert!(app.notice.is_none(), "images for this machine need no warning");
    }

    #[test]
    fn numbering_restarts_once_no_placeholder_is_left() {
        let mut app = composing(Vec::new());
        app.update(image(A), at(1));
        app.update(press_with(KeyCode::Char('u'), KeyModifiers::CONTROL), at(1));
        app.update(image(B), at(1));
        assert_eq!(app.composer.task.text(), "[Image #1] ");
        assert_eq!(app.composer.images, [B]);
    }

    #[test]
    fn ctrl_s_keeps_the_images_for_the_next_launch() {
        let mut app = composing(Vec::new());
        app.update(image(A), at(1));
        type_text(&mut app, "twice");
        let first = app.update(press_with(KeyCode::Char('s'), KeyModifiers::CONTROL), at(2));
        let second = app.update(press(KeyCode::Enter), at(3));
        assert_eq!(launched_task(&first), format!("{A} twice"));
        assert_eq!(launched_task(&second), format!("{A} twice"));
    }

    #[test]
    fn the_history_brings_images_back_as_placeholders() {
        let mut app = composing(vec![format!("{A} older task")]);
        app.update(image(B), at(1));
        type_text(&mut app, "draft");
        app.update(press_with(KeyCode::Char('p'), KeyModifiers::CONTROL), at(1));
        assert_eq!(app.composer.task.text(), "[Image #1] older task");
        assert_eq!(app.composer.images, [A]);
        app.update(press_with(KeyCode::Char('n'), KeyModifiers::CONTROL), at(1));
        assert_eq!(app.composer.task.text(), "[Image #1] draft", "the draft keeps its own image");
        let effects = app.update(press(KeyCode::Enter), at(2));
        assert_eq!(launched_task(&effects), format!("{B} draft"));
    }

    #[test]
    fn clipboard_text_is_pasted_and_problems_are_said() {
        let mut app = composing(Vec::new());
        app.update(Input::Clipboard(Ok(Clip::Text("copied\ntext".into()))), at(1));
        assert_eq!(app.composer.task.text(), "copied\ntext");
        app.update(Input::Clipboard(Ok(Clip::Empty)), at(1));
        assert_eq!(app.notice.as_ref().unwrap().text, "The clipboard is empty.");
        app.update(Input::Clipboard(Err("Install wl-clipboard to paste images.".into())), at(1));
        let notice = app.notice.as_ref().unwrap();
        assert_eq!(notice.text, "Install wl-clipboard to paste images.");
        assert_eq!(notice.kind, NoticeKind::Error);
        assert_eq!(app.composer.task.text(), "copied\ntext");
    }

    #[test]
    fn a_clipboard_read_that_lands_after_leaving_is_dropped() {
        let mut app = composing(Vec::new());
        app.update(press(KeyCode::Esc), at(1));
        assert!(app.update(image(A), at(1)).is_empty());
        assert!(
            app.update(Input::Clipboard(Ok(Clip::Text("ls\n".into()))), at(1)).is_empty(),
            "never typed into an agent"
        );
        assert_eq!(app.composer.task.text(), "");
        assert!(app.composer.images.is_empty());
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_reply_takes_images_too() {
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('j')), at(1));
        app.update(press(KeyCode::Char('r')), at(1));
        type_text(&mut app, "see");
        let effects = app.update(press_with(KeyCode::Char('v'), KeyModifiers::CONTROL), at(1));
        assert_eq!(effects, vec![Effect::ReadClipboard]);
        app.update(image(A), at(1));
        assert_eq!(app.reply.as_ref().unwrap().editor.text(), "see [Image #1] ");
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert_eq!(
            effects,
            vec![Effect::Prompt {
                machine: LOCAL.into(),
                pane_id: "w3:p1".into(),
                title: "Docs".into(),
                text: format!("see {A}"),
            }]
        );
    }

    #[test]
    fn images_sent_to_another_machine_say_they_stay_here() {
        let mut app = fleet();
        app.cursor = Some("studio/w2:p1".into());
        app.update(press(KeyCode::Char('r')), at(1));
        app.update(image(A), at(1));
        let effects = app.update(press(KeyCode::Enter), at(1));
        assert!(matches!(&effects[..], [Effect::Prompt { machine, text, .. }] if machine == "studio" && text == A));
        let notice = app.notice.as_ref().expect("a warning");
        assert_eq!(notice.text, "Pasted images stay on this machine: Mac Studio only gets their paths.");

        app.notice = None;
        app.cursor = Some("studio/w2:p1".into());
        app.update(press(KeyCode::Char('r')), at(1));
        type_text(&mut app, "no image");
        app.update(press(KeyCode::Enter), at(1));
        assert!(app.notice.is_none(), "plain text goes quietly");
    }

    #[test]
    fn an_open_agent_still_gets_ctrl_v_itself() {
        // Claude Code, Codex and Pi read the clipboard on Ctrl+V on their own.
        let (mut app, _) = loaded();
        app.update(press(KeyCode::Enter), at(1));
        let effects = app.update(press_with(KeyCode::Char('V'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), at(1));
        assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(vec![0x16]) }]);
    }
}
