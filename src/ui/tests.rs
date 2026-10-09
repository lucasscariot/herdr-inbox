use std::time::{Duration, SystemTime};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use std::collections::HashMap;

use super::*;
use crate::app::MachineInfo;
use crate::app::{Input, StreamState};
use crate::git::Checkout;
use crate::herdr::terminal::{Frame, Message};
use crate::herdr::types::{AgentInfo, SessionSnapshot};
use crate::threads::LOCAL;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn agent(pane: &str, status: AgentStatus, title: &str, cwd: &str, kind: &str) -> AgentInfo {
    let workspace = pane.split(':').next().unwrap_or(pane);
    AgentInfo {
        pane_id: pane.into(),
        workspace_id: workspace.into(),
        tab_id: format!("{workspace}:t1"),
        terminal_id: String::new(),
        agent_status: status,
        agent: Some(kind.into()),
        display_agent: None,
        name: None,
        title: None,
        terminal_title_stripped: Some(title.into()),
        cwd: Some(cwd.into()),
        foreground_cwd: None,
        tokens: Default::default(),
        state_change_seq: 0,
        focused: false,
    }
}

fn checkouts() -> HashMap<String, Checkout> {
    [
        ("/w/cockpit", "cockpit", "fix-login-redirect"),
        ("/w/api", "api", "main"),
        ("/w/site", "site", "feat/a-very-long-branch-name-that-will-not-fit"),
    ]
    .into_iter()
    .map(|(path, repo, branch)| {
        (path.to_string(), Checkout { repo: repo.into(), branch: Some(branch.into()), root: path.into() })
    })
    .collect()
}

fn snapshot(machine: &str, agents: Vec<AgentInfo>) -> Input {
    Input::Snapshot {
        machine: machine.into(),
        snapshot: SessionSnapshot { agents, ..SessionSnapshot::default() },
        checkouts: checkouts(),
    }
}

fn loaded(width: u16, height: u16) -> App {
    let mut app = App::new(width, height, vec![]);
    app.update(
        snapshot(
            LOCAL,
            vec![
                agent("w1:p1", AgentStatus::Blocked, "Fix the login redirect loop on mobile", "/w/cockpit", "claude"),
                agent("w2:p1", AgentStatus::Working, "Add invoice export", "/w/api", "codex"),
                agent("w3:p1", AgentStatus::Done, "Review navigation", "/w/site", "opencode"),
                agent("w4:p1", AgentStatus::Idle, "lucas@host:~", "/tmp/scratch", "pi"),
            ],
        ),
        at(0),
    );
    app
}

fn render(app: &App, now: SystemTime) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(app.layout.width, app.layout.height)).unwrap();
    terminal.draw(|frame| draw(frame, app, &Palette::default(), now)).unwrap();
    terminal
}

fn text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn find(terminal: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        let row: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect();
        if let Some(byte) = row.find(needle) {
            return (row[..byte].width() as u16, y);
        }
    }
    panic!("{needle:?} not on screen:\n{}", text(terminal));
}

fn press(app: &mut App, code: KeyCode) {
    app.update(
        Input::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }),
        at(1),
    );
}

#[test]
fn the_inbox_groups_threads_by_what_they_need() {
    let app = loaded(100, 24);
    insta::assert_snapshot!(text(&render(&app, at(0))));
}

#[test]
fn status_colors_carry_meaning() {
    let app = loaded(100, 24);
    let terminal = render(&app, at(0));
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[find(&terminal, "● input")].fg, palette.red);
    assert_eq!(buffer[find(&terminal, "◐ working")].fg, palette.yellow);
    assert_eq!(buffer[find(&terminal, "✓ ready")].fg, palette.teal);
    assert_eq!(buffer[find(&terminal, "NEEDS INPUT")].fg, palette.red);
    assert_eq!(buffer[find(&terminal, "⎇ main")].fg, palette.mauve);
}

#[test]
fn the_cursor_row_is_highlighted_in_the_list_and_the_open_title_is_accented() {
    let mut app = loaded(100, 24);
    press(&mut app, KeyCode::Char('j'));
    let terminal = render(&app, at(0));
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    let (_, cursor_y) = find(&terminal, "Review navigation");
    assert_eq!(buffer[(5, cursor_y)].bg, palette.active_row_bg);
    let (x, open_y) = find(&terminal, "Fix the login");
    assert_eq!(buffer[(x, open_y)].fg, palette.accent, "the open thread keeps an accent title");
    assert_ne!(buffer[(5, open_y)].bg, palette.active_row_bg);
}

#[test]
fn with_the_agent_focused_the_open_thread_is_highlighted() {
    let mut app = loaded(100, 24);
    press(&mut app, KeyCode::Enter);
    let terminal = render(&app, at(0));
    let buffer = terminal.backend().buffer();
    let (_, y) = find(&terminal, "Fix the login");
    assert_eq!(buffer[(5, y)].bg, Palette::default().active_row_bg);
    assert!(text(&terminal).contains("AGENT"));
}

#[test]
fn long_titles_and_branches_are_clipped_with_an_ellipsis() {
    let app = loaded(70, 24);
    let screen = text(&render(&app, at(0)));
    assert!(screen.contains("▎Fix the login redirect lo… │"), "{screen}");
    assert!(screen.contains("▎⎇ feat/a-very-long-branch… │"), "{screen}");
}

#[test]
fn ages_show_once_a_change_was_seen() {
    let mut app = loaded(100, 24);
    app.update(
        Input::Status {
            machine: LOCAL.into(),
            change: crate::herdr::types::AgentStatusChange {
                pane_id: "w2:p1".into(),
                agent_status: AgentStatus::Blocked,
                agent: None,
                display_agent: None,
                title: None,
            },
        },
        at(100),
    );
    let screen = text(&render(&app, at(100 + 125)));
    assert!(screen.contains("Codex") && screen.contains(" 2m"), "{screen}");
}

#[test]
fn the_live_terminal_is_drawn_with_its_cursor() {
    let mut app = loaded(100, 24);
    press(&mut app, KeyCode::Enter);
    app.update(
        Input::Terminal {
            generation: 1,
            message: Message::Frame(Frame {
                seq: 1,
                width: 69,
                height: 23,
                full: true,
                bytes: b"\x1b[1;1H\xe2\x9c\xbb Claude Code\x1b[3;1H> fix it\x1b[3;9H".to_vec(),
            }),
        },
        at(1),
    );
    let mut terminal = render(&app, at(1));
    let screen = text(&terminal);
    assert!(screen.lines().next().unwrap().ends_with("│✻ Claude Code"), "{screen}");
    let area = app.layout.terminal;
    terminal.backend_mut().assert_cursor_position((area.x + 8, area.y + 2));
}

#[test]
fn no_cursor_while_the_list_has_focus() {
    let mut app = loaded(100, 24);
    app.update(
        Input::Terminal {
            generation: 1,
            message: Message::Frame(Frame { seq: 1, width: 69, height: 23, full: true, bytes: b"x".to_vec() }),
        },
        at(1),
    );
    assert_eq!(cursor(&app), None);
    press(&mut app, KeyCode::Tab);
    assert!(cursor(&app).is_some());
}

#[test]
fn opening_a_thread_shows_a_placeholder_until_the_first_frame() {
    let app = loaded(100, 24);
    assert_eq!(app.open.as_ref().unwrap().stream, StreamState::Attaching);
    let screen = text(&render(&app, at(0)));
    assert!(screen.contains("Opening Fix the login redirect loop on mobile…"), "{screen}");
}

#[test]
fn a_taken_over_thread_offers_to_bring_it_back() {
    let mut app = loaded(100, 24);
    app.update(Input::Terminal { generation: 1, message: Message::Closed { reason: Some(TAKEN_OVER.into()) } }, at(1));
    insta::assert_snapshot!(text(&render(&app, at(1))));
}

#[test]
fn a_stream_that_ended_for_another_reason_says_why() {
    let mut app = loaded(100, 24);
    app.update(
        Input::Terminal { generation: 1, message: Message::Closed { reason: Some("server is shutting down".into()) } },
        at(1),
    );
    let screen = text(&render(&app, at(1)));
    assert!(screen.contains("The live view of this thread stopped."));
    assert!(screen.contains("server is shutting down"));
}

#[test]
fn archiving_asks_on_the_thread_itself() {
    let mut app = loaded(100, 24);
    press(&mut app, KeyCode::Char('x'));
    let screen = text(&render(&app, at(1)));
    assert!(screen.contains("Archive this thread? y / n"), "{screen}");
    assert!(screen.contains("y archive  n keep"), "{screen}");
}

#[test]
fn notices_replace_nothing_but_the_right_of_the_status_bar() {
    let mut app = loaded(100, 24);
    app.update(Input::Archived { title: "Docs".into(), result: Err("workspace not found".into()) }, at(1));
    let terminal = render(&app, at(1));
    let last = text(&terminal).lines().last().unwrap().to_string();
    assert!(last.contains("THREADS") && last.ends_with("Could not archive “Docs”: workspace not found"), "{last}");
    assert_eq!(terminal.backend().buffer()[find(&terminal, "Could not")].fg, Palette::default().red);
}

#[test]
fn an_empty_session_explains_where_threads_come_from() {
    let mut app = App::new(100, 20, vec![]);
    app.update(snapshot(LOCAL, vec![]), at(0));
    insta::assert_snapshot!(text(&render(&app, at(0))));
}

#[test]
fn without_a_server_the_screen_offers_to_start_one() {
    let mut app = App::new(80, 12, vec![]);
    app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::NoServer }, at(0));
    insta::assert_snapshot!(text(&render(&app, at(0))));
}

#[test]
fn a_lost_connection_is_visible_everywhere() {
    let mut app = loaded(100, 20);
    app.update(
        Input::Connection { machine: LOCAL.into(), connection: Connection::Lost("server closed the socket".into()) },
        at(1),
    );
    let screen = text(&render(&app, at(1)));
    assert!(screen.contains("reconnecting…"));
    assert!(screen.contains("Lost the connection to Herdr. Retrying…"));
    assert!(screen.lines().last().unwrap().ends_with("✗ server closed the socket"));
}

#[test]
fn narrow_and_tiny_windows_render_without_panicking() {
    for (w, h) in [(1, 1), (10, 3), (30, 8), (44, 16), (200, 60)] {
        let mut app = loaded(w, h);
        press(&mut app, KeyCode::Char('x'));
        let _ = render(&app, at(0));
        app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::NoServer }, at(0));
        let _ = render(&app, at(0));
    }
}

#[test]
fn centred_messages_wrap_instead_of_being_cut() {
    let mut app = App::new(60, 12, vec![]);
    app.update(snapshot(LOCAL, vec![]), at(0));
    let screen = text(&render(&app, at(0)));
    let right: Vec<&str> = screen.lines().map(|l| l.split('│').nth(1).unwrap_or("").trim()).collect();
    let message = right.iter().filter(|l| !l.is_empty()).copied().collect::<Vec<_>>().join(" ");
    assert_eq!(message, "Agent threads you start in Herdr appear on the left.", "{screen}");
}

#[test]
fn ages_round_down_to_the_largest_unit() {
    let now = at(1_000_000);
    let ago = |secs: u64| age(at(1_000_000 - secs), now);
    assert_eq!(ago(0), "now");
    assert_eq!(ago(4), "now");
    assert_eq!(ago(5), "5s");
    assert_eq!(ago(59), "59s");
    assert_eq!(ago(60), "1m");
    assert_eq!(ago(3599), "59m");
    assert_eq!(ago(3600), "1h");
    assert_eq!(ago(86_399), "23h");
    assert_eq!(ago(86_400), "1d");
    assert_eq!(age(at(10), at(5)), "now", "a clock that went backwards is not negative");
}

#[test]
fn clip_respects_display_width() {
    assert_eq!(clip("hello", 5), "hello");
    assert_eq!(clip("hello", 4), "hel…");
    assert_eq!(clip("hello", 1), "…");
    assert_eq!(clip("hello", 0), "");
    assert_eq!(clip("日本語テキスト", 7), "日本語…");
    assert_eq!(clip("日本語", 6), "日本語");
}

fn fleet(width: u16, height: u16) -> App {
    let mut app = App::new(
        width,
        height,
        vec![
            MachineInfo { id: "studio".into(), label: "Mac Studio".into() },
            MachineInfo { id: "book".into(), label: "MacBook".into() },
        ],
    );
    app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::NoServer }, at(0));
    app.update(
        snapshot("studio", vec![agent("w1:p1", AgentStatus::Blocked, "Ship the release", "/w/api", "codex")]),
        at(0),
    );
    app.update(Input::Connection { machine: "book".into(), connection: Connection::Lost("timed out".into()) }, at(0));
    app
}

#[test]
fn remote_threads_name_their_machine() {
    let app = fleet(110, 16);
    let screen = text(&render(&app, at(0)));
    assert!(screen.contains("⎇ main · Mac Studio · Codex"), "{screen}");
}

#[test]
fn the_status_bar_shows_every_machine_and_how_to_start_a_missing_local_server() {
    let app = fleet(120, 16);
    let terminal = render(&app, at(0));
    let last = text(&terminal).lines().last().unwrap().to_string();
    assert!(last.ends_with("○ Local s start  ● Mac Studio  ✗ MacBook"), "{last}");
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[find(&terminal, "● Mac Studio")].fg, palette.green);
    assert_eq!(buffer[find(&terminal, "✗ MacBook")].fg, palette.red);
    assert_eq!(buffer[find(&terminal, "○ Local")].fg, palette.red);
}

#[test]
fn a_missing_local_server_is_not_a_full_screen_when_other_machines_exist() {
    let app = fleet(110, 16);
    let screen = text(&render(&app, at(0)));
    assert!(!screen.contains("No Herdr server is running."), "{screen}");
    assert!(screen.contains("Ship the release"), "{screen}");
}

#[test]
fn a_notice_takes_the_place_of_the_machine_strip() {
    let mut app = fleet(110, 16);
    app.update(Input::Archived { title: "x".into(), result: Ok(()) }, at(1));
    let last = text(&render(&app, at(1))).lines().last().unwrap().to_string();
    assert!(last.ends_with("Archived “x”") && !last.contains("Mac Studio"), "{last}");
}
