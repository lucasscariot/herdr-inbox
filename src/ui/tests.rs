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
    render_with_palette(app, now, &Palette::default())
}

fn render_with_palette(app: &App, now: SystemTime, palette: &Palette) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(app.layout.width, app.layout.height)).unwrap();
    terminal.draw(|frame| draw(frame, app, palette, now)).unwrap();
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
fn idle_threads_and_dividers_stay_faint() {
    let app = loaded(100, 24);
    let terminal = render(&app, at(0));
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    let (x, y) = find(&terminal, "○ idle");
    let idle = &buffer[(x, y)];
    assert_eq!(idle.fg, palette.overlay1);
    assert!(idle.modifier.contains(Modifier::DIM), "an idle badge is the quietest status");
    assert!(buffer[(0, y)].modifier.contains(Modifier::DIM), "and so is its marker");
    let heading = &buffer[find(&terminal, "IDLE")];
    assert!(heading.modifier.contains(Modifier::DIM | Modifier::BOLD), "the idle group heading too");
    assert!(
        !buffer[find(&terminal, "◐ working")].modifier.contains(Modifier::DIM),
        "live statuses keep their full tone"
    );
    let separator = &buffer[(app.layout.sidebar.width, 5)];
    assert_eq!(separator.symbol(), "│");
    assert_eq!(separator.fg, palette.surface1);
    assert!(separator.modifier.contains(Modifier::DIM), "the sidebar divider is a hairline, not a white bar");
}

#[test]
fn the_header_splits_stats_actions_and_list_with_hairlines() {
    let mut app = App::new(100, 24, vec![]);
    app.update(
        snapshot(
            LOCAL,
            vec![
                agent("w1:p1", AgentStatus::Blocked, "Fix the login", "/w/cockpit", "claude"),
                agent("w2:p1", AgentStatus::Working, "Add invoice export", "/w/api", "codex"),
                agent("w3:p1", AgentStatus::Working, "Write the docs", "/w/api", "codex"),
            ],
        ),
        at(0),
    );
    let terminal = render(&app, at(0));
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    let screen = text(&terminal);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[1].contains("● 1  ✓ 0  ◐ 2  ○ 0") && lines[1].contains("3 threads"), "{screen}");
    assert!(!lines[1].contains("·"), "no unknown group while it is empty: {screen}");
    for row in [2, 5] {
        let rule = &buffer[(1, row)];
        assert_eq!(rule.symbol(), "─", "{screen}");
        assert_eq!(rule.fg, palette.surface1);
        assert!(rule.modifier.contains(Modifier::DIM), "rules are hairlines");
    }
    assert!(lines[3].contains("+  New thread") && lines[4].contains("/  Filter threads"), "{screen}");
    assert!(lines[6].contains("NEEDS INPUT"), "the list starts under the second rule: {screen}");
    let (x, y) = find(&terminal, "● 1");
    assert_eq!(buffer[(x, y)].fg, palette.red);
    let (x, y) = find(&terminal, "✓ 0");
    assert!(buffer[(x, y)].modifier.contains(Modifier::DIM), "an empty group stays faint");
}

#[test]
fn the_selected_discussion_is_highlighted_and_accented_while_browsing() {
    let mut app = loaded(100, 24);
    press(&mut app, KeyCode::Char('j'));
    let terminal = render(&app, at(0));
    let palette = Palette::default();
    let buffer = terminal.backend().buffer();
    // The terminal's placeholder names the thread too; the sidebar row has the edge.
    let (x, cursor_y) = find(&terminal, "▎Review navigation");
    let x = x + 1;
    assert_eq!(buffer[(5, cursor_y)].bg, palette.active_row_bg);
    assert_eq!(buffer[(x, cursor_y)].fg, palette.accent, "the selected discussion is already open");
    let (x, previous_y) = find(&terminal, "Fix the login");
    assert_ne!(buffer[(x, previous_y)].fg, palette.accent);
    assert_ne!(buffer[(5, previous_y)].bg, palette.active_row_bg);
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
    assert!(
        screen.lines().any(|line| line.contains("Fix the login") && (line.contains("… │") || line.contains("… ▐"))),
        "{screen}"
    );
    assert!(
        screen
            .lines()
            .any(|line| line.contains("OpenCode · ⎇ feat/") && (line.contains("… │") || line.contains("… ▐"))),
        "{screen}"
    );
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
    assert!(screen.contains("Archive this thread?") && screen.contains("↵/esc"), "{screen}");
    assert!(screen.contains("⌫/↵ archive  esc cancel"), "{screen}");
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
    // Short enough that no orbit sits above the message.
    let mut app = App::new(60, 10, vec![]);
    app.update(snapshot(LOCAL, vec![]), at(0));
    let screen = text(&render(&app, at(0)));
    let right: Vec<&str> = screen.lines().map(|l| l.split('│').nth(1).unwrap_or("").trim()).collect();
    let message = right.iter().filter(|l| !l.is_empty()).copied().collect::<Vec<_>>().join(" ");
    assert_eq!(
        message, "Start a thread with n or + New thread. Agents already running in Herdr appear on the left.",
        "{screen}"
    );
}

#[test]
fn the_divider_shows_a_scroll_thumb_and_clickable_filter_affordance() {
    let mut app = App::new(100, 16, vec![]);
    app.update(
        snapshot(
            LOCAL,
            (0..20).map(|i| agent(&format!("w{i}:p1"), AgentStatus::Working, "Task", "/w/api", "codex")).collect(),
        ),
        at(0),
    );
    let terminal = render(&app, at(0));
    let x = app.layout.sidebar.right();
    assert_eq!(terminal.backend().buffer()[(x, app.layout.list.y)].symbol(), "▐");
    assert!(text(&terminal).contains("/  Filter threads"));
    app.layout.scroll(1000);
    let terminal = render(&app, at(0));
    assert_eq!(terminal.backend().buffer()[(x, app.layout.list.bottom() - 1)].symbol(), "▐");
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
    assert!(screen.contains("Codex · Mac Studio · ⎇ main"), "{screen}");
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

mod composer {
    use super::*;
    use crate::discovery::{Catalog, CheckoutEntry, Choice as ModelChoice, Inventory, Project};
    use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::collections::BTreeMap;

    fn key(app: &mut App, code: KeyCode) {
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

    fn composing(width: u16, height: u16) -> App {
        let mut app = loaded(width, height);
        key(&mut app, KeyCode::Char('n'));
        let inventory = Inventory {
            projects: vec![Project {
                name: "cockpit".into(),
                path: "/w/cockpit".into(),
                branch: "main".into(),
                checkouts: vec![
                    CheckoutEntry { path: "/w/cockpit".into(), branch: "main".into(), linked: false },
                    CheckoutEntry { path: "/h/wt/fix".into(), branch: "fix".into(), linked: true },
                ],
            }],
            harnesses: vec!["claude".into()],
            models: BTreeMap::from([(
                "claude".to_string(),
                Catalog {
                    choices: vec![ModelChoice { id: "opus".into(), label: "Opus".into() }],
                    selectable: true,
                    default: "opus".into(),
                    thinking_flag: "--effort".into(),
                    thinking: vec!["high".into()],
                    ..Catalog::default()
                },
            )]),
            models_at: 0,
            models_revision: crate::discovery::MODELS_REVISION,
        };
        app.update(Input::Inventory { machine: LOCAL.into(), result: Ok(inventory) }, at(1));
        for c in "Fix the login redirect loop".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        app
    }

    #[test]
    fn regression_clicking_the_task_restores_focus_and_places_the_cursor() {
        let mut app = composing(110, 30);
        key(&mut app, KeyCode::Tab);
        let terminal = render(&app, at(1));
        let (x, y) = find(&terminal, "Fix the login redirect loop");
        let effects = app.update(
            Input::Mouse(ratatui::crossterm::event::MouseEvent {
                kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
                column: x + 4,
                row: y,
                modifiers: KeyModifiers::NONE,
            }),
            at(1),
        );
        assert!(effects.is_empty(), "a composer click never reaches an agent");
        assert_eq!(app.composer.field, crate::app::Field::Task);
        assert_eq!(app.composer.task.cursor(), 4);
        key(&mut app, KeyCode::Char('X'));
        assert_eq!(app.composer.task.text(), "Fix Xthe login redirect loop");
    }

    #[test]
    fn regression_the_composer_uses_the_panel_background() {
        let app = composing(110, 30);
        let terminal = render(&app, at(1));
        let (x, y) = find(&terminal, "╭");
        assert_eq!(terminal.backend().buffer()[(x, y)].bg, Palette::default().panel_bg);
    }

    #[test]
    fn the_composer_replaces_the_agent_and_lists_every_choice() {
        let app = composing(110, 30);
        insta::assert_snapshot!(text(&render(&app, at(1))));
    }

    #[test]
    fn the_cursor_sits_at_the_end_of_the_task() {
        let app = composing(110, 30);
        let mut terminal = render(&app, at(1));
        let (x, y) = find(&terminal, "Fix the login redirect loop");
        terminal.backend_mut().assert_cursor_position((x + "Fix the login redirect loop".len() as u16, y));
    }

    #[test]
    fn a_picker_floats_under_its_field_with_the_query_cursor() {
        let mut app = composing(110, 30);
        key(&mut app, KeyCode::F(9));
        key(&mut app, KeyCode::Char('f'));
        let mut terminal = render(&app, at(1));
        let screen = text(&terminal);
        assert!(screen.contains(" Workspace "), "{screen}");
        assert!(screen.contains("› f"), "{screen}");
        assert!(screen.contains("New worktree named f"), "{screen}");
        let (x, y) = find(&terminal, "› f");
        terminal.backend_mut().assert_cursor_position((x + 3, y));
    }

    #[test]
    fn launches_and_discovery_errors_are_listed() {
        let mut app = composing(110, 34);
        key(&mut app, KeyCode::Enter);
        app.update(Input::Inventory { machine: LOCAL.into(), result: Err("python3: command not found".into()) }, at(2));
        let screen = text(&render(&app, at(2)));
        assert!(screen.contains("✗ Local: python3: command not found"), "{screen}");
        assert!(screen.contains("LAUNCHES"), "{screen}");
        assert!(screen.contains("⟳ cockpit · Claude  Fix the login redirect loop"), "{screen}");
    }

    #[test]
    fn a_send_error_shows_under_the_task() {
        let mut app = loaded(110, 30);
        key(&mut app, KeyCode::Char('n'));
        key(&mut app, KeyCode::Enter);
        let screen = text(&render(&app, at(1)));
        assert!(screen.contains("Write a task first."), "{screen}");
    }

    #[test]
    fn the_task_frame_is_a_softened_accent_with_focus_and_a_hairline_without() {
        let mut app = composing(110, 30);
        let palette = Palette::default();
        let terminal = render(&app, at(1));
        let buffer = terminal.backend().buffer();
        let (x, y) = find(&terminal, "╭");
        let focused = &buffer[(x, y)];
        assert_eq!(focused.fg, palette.accent);
        assert!(focused.modifier.contains(Modifier::DIM), "a full-strength accent box drowns the task");
        key(&mut app, KeyCode::Tab);
        let terminal = render(&app, at(1));
        let blurred = &terminal.backend().buffer()[(x, y)];
        assert_eq!(blurred.fg, palette.surface1);
        assert!(blurred.modifier.contains(Modifier::DIM), "without focus the frame is a hairline");
        assert_eq!(blurred.symbol(), "╭");
    }

    #[test]
    fn popup_borders_titles_and_backgrounds_follow_dark_light_and_custom_themes() {
        for config in [
            "[theme]\nname = \"catppuccin\"",
            "[theme]\nname = \"catppuccin-latte\"",
            "[theme]\nname = \"terminal\"",
            "[theme.custom]\naccent = \"#123456\"\nsubtext0 = \"#abcdef\"\npanel_bg = \"#020304\"\nsurface0 = \"#050607\"",
        ] {
            let palette = crate::theme::from_herdr_config(config);
            let mut app = composing(110, 30);
            key(&mut app, KeyCode::F(4));
            let layout =
                crate::app::ComposerLayout::new(app.layout.terminal, &app.composer, &app.composer_context()).unwrap();
            let popup = layout.picker.unwrap().popup;
            let terminal = render_with_palette(&app, at(1), &palette);
            let buffer = terminal.backend().buffer();
            for (x, y) in [
                (popup.x, popup.y),
                (popup.right() - 1, popup.y),
                (popup.x, popup.bottom() - 1),
                (popup.right() - 1, popup.bottom() - 1),
            ] {
                assert_eq!(buffer[(x, y)].fg, palette.accent);
                assert_eq!(buffer[(x, y)].bg, palette.surface0);
                assert!(buffer[(x, y)].modifier.contains(Modifier::DIM));
            }
            assert_eq!(
                buffer[(popup.x + 2, popup.y)].fg,
                palette.subtext0,
                "titles must not use terminal-default white"
            );
            assert_eq!(buffer[(layout.task_box.x, layout.task_box.y)].bg, palette.panel_bg);
        }
    }

    #[test]
    fn a_long_task_does_not_hide_workspace_or_send_in_a_standard_terminal() {
        let mut app = composing(100, 24);
        app.composer.task.set(&"A long task with several lines\n".repeat(12));
        let screen = text(&render(&app, at(1)));
        assert!(screen.contains("Workspace"), "{screen}");
        assert!(screen.contains("↵ send"), "{screen}");
    }

    #[test]
    fn long_picker_queries_keep_the_cursor_inside_the_popup() {
        let mut app = composing(70, 24);
        key(&mut app, KeyCode::F(4));
        app.update(Input::Paste("a-model-id-that-is-much-longer-than-the-popup".into()), at(1));
        let mut terminal = render(&app, at(1));
        let cursor = terminal.get_cursor_position().unwrap();
        let layout =
            crate::app::ComposerLayout::new(app.layout.terminal, &app.composer, &app.composer_context()).unwrap();
        let query = layout.picker.unwrap().query;
        assert!(cursor.x < query.right() && cursor.x >= query.x);
    }

    #[test]
    fn the_new_thread_button_sits_in_the_sidebar_and_opens_the_composer() {
        let mut app = loaded(100, 24);
        let screen = text(&render(&app, at(0)));
        assert!(screen.lines().nth(3).unwrap().contains("+  New thread"), "{screen}");
        let button = app.layout.new_button;
        app.update(
            Input::Mouse(ratatui::crossterm::event::MouseEvent {
                kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
                column: button.x + 3,
                row: button.y,
                modifiers: KeyModifiers::NONE,
            }),
            at(1),
        );
        assert_eq!(app.focus, crate::app::Focus::Composer);
    }

    #[test]
    fn tiny_windows_render_the_composer_without_panicking() {
        for (w, h) in [(30, 6), (60, 10), (44, 16)] {
            let mut app = composing(w, h);
            key(&mut app, KeyCode::F(4));
            let _ = render(&app, at(1));
        }
    }
}

mod conveniences {
    use super::*;
    use crate::launch::{Record, Stage};
    use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(app: &mut App, code: KeyCode) {
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

    fn record(id: &str, pane: Option<&str>, stage: Stage, unverified: bool) -> Record {
        Record {
            id: id.into(),
            machine_id: LOCAL.into(),
            machine_label: "Local".into(),
            project: "cockpit".into(),
            repo: "/w/cockpit".into(),
            harness: "claude".into(),
            model: String::new(),
            thinking: String::new(),
            title: "Ship the release notes".into(),
            task: "Ship the release notes".into(),
            agent_name: "t".into(),
            created_at: 0,
            stage,
            workspace: "worktree".into(),
            branch: "release-notes".into(),
            cwd: String::new(),
            workspace_id: None,
            pane_id: pane.map(str::to_string),
            tab_id: None,
            unverified,
            failed_stage: None,
            error: Some("expected claude, detected bash".into()),
            comparison: None,
        }
    }

    #[test]
    fn launch_notes_replace_the_branch_line() {
        let mut app = loaded(110, 40);
        app.update(
            Input::Journals(vec![
                record("f", None, Stage::NeedsAttention, false),
                record("w", Some("w2:p1"), Stage::StartupBlocked, false),
                record("u", Some("w4:p1"), Stage::Submitted, true),
            ]),
            at(1),
        );
        let terminal = render(&app, at(1));
        let screen = text(&terminal);
        assert!(
            screen.lines().any(|line| line.contains("failed: expected claude") && line.ends_with("… │")),
            "{screen}"
        );
        assert!(screen.contains("waiting: answer its prompt"), "{screen}");
        assert!(screen.contains("sent, not confirmed yet"), "{screen}");
        assert_eq!(terminal.backend().buffer()[find(&terminal, "failed: expected")].fg, Palette::default().red);
        assert_eq!(terminal.backend().buffer()[find(&terminal, "waiting: answer")].fg, Palette::default().yellow);
    }

    #[test]
    fn the_reply_box_floats_over_the_terminal_with_its_cursor() {
        let mut app = loaded(110, 30);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('r'));
        for c in "add tests".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let mut terminal = render(&app, at(1));
        let screen = text(&terminal);
        assert!(screen.contains("Reply to “Add invoice export”"), "{screen}");
        assert!(screen.contains("↵ send · ⇧↵ new line · esc cancel"), "{screen}");
        assert!(screen.lines().last().unwrap().contains("REPLY"));
        let (x, y) = find(&terminal, "add tests");
        terminal.backend_mut().assert_cursor_position((x + 9, y));
    }

    #[test]
    fn the_filter_shows_in_the_header() {
        let mut app = loaded(110, 30);
        key(&mut app, KeyCode::Char('/'));
        for c in "invoice".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let screen = text(&render(&app, at(1)));
        let search = screen.lines().nth(4).unwrap();
        assert!(search.contains("/  invoice▏") && search.contains("1 of 4"), "{screen}");
        assert!(screen.lines().nth(1).unwrap().contains("4 threads"), "the stats count the whole inbox: {screen}");
        assert!(screen.contains("Add invoice export"));
        assert!(!screen.contains("Review navigation"), "{screen}");
        assert!(screen.lines().last().unwrap().contains("FILTER"));
    }
}

mod voice {
    use super::*;
    use crate::app::{Phase, SpeechStatus};
    use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn ctrl_t(app: &mut App) {
        app.update(
            Input::Key(KeyEvent {
                code: KeyCode::Char('t'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
            at(0),
        );
    }

    #[test]
    fn recording_shows_a_live_meter_and_the_keys() {
        let mut app = loaded(120, 24);
        app.update(Input::Speech(SpeechStatus { ready: Some("Groq Whisper".into()), tools: vec![] }), at(0));
        ctrl_t(&mut app);
        app.update(Input::DictationStarted(Ok(())), at(0));
        app.update(
            Input::Levels { levels: vec![0.0, 0.3, 0.7, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], quiet: false },
            at(0),
        );
        let terminal = render(&app, at(65));
        let last = text(&terminal).lines().last().unwrap().to_string();
        assert!(last.starts_with(" ● REC 1:05 ▁▂▆█"), "{last}");
        assert!(last.contains("↵ send  ⌃T type  esc discard"), "{last}");
        assert!(last.ends_with("to “Fix the login redirect loop on mobile”"), "{last}");
        let palette = Palette::default();
        let buffer = terminal.backend().buffer();
        let (x, y) = find(&terminal, "● REC");
        assert_eq!(buffer[(x, y)].fg, palette.red);
        assert_eq!(buffer[(x + 13, y)].fg, palette.accent, "0.7 runs warm");
        assert_eq!(buffer[(x + 14, y)].fg, palette.red, "a full band runs hot");
        assert_eq!(buffer[(x + 12, y)].fg, palette.green);
    }

    #[test]
    fn silence_and_transcribing_are_said_plainly() {
        let mut app = loaded(120, 24);
        app.update(Input::Speech(SpeechStatus { ready: Some("Groq Whisper".into()), tools: vec![] }), at(0));
        ctrl_t(&mut app);
        app.update(Input::DictationStarted(Ok(())), at(0));
        app.update(Input::Levels { levels: vec![0.0; 12], quiet: true }, at(0));
        assert!(text(&render(&app, at(5))).lines().last().unwrap().ends_with("no sound is reaching the microphone"));
        app.update(
            Input::Key(KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
            at(5),
        );
        assert_eq!(app.dictation.as_ref().unwrap().phase, Phase::Transcribing);
        assert!(text(&render(&app, at(6))).lines().last().unwrap().starts_with(" ⟳ Transcribing…"));
    }

    #[test]
    fn the_menu_lists_services_and_hides_a_typed_key() {
        let mut app = loaded(110, 30);
        app.update(
            Input::Key(KeyEvent {
                code: KeyCode::F(10),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
            at(0),
        );
        let screen = text(&render(&app, at(0)));
        assert!(screen.contains(" Dictation "), "{screen}");
        assert!(screen.contains("No transcription yet. Connect one:"), "{screen}");
        assert!(screen.contains("Connect Groq Whisper") && screen.contains("free tier · fastest"), "{screen}");
        assert!(screen.contains("Build whisper.cpp here"));
        app.update(
            Input::Key(KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
            at(0),
        );
        for c in "gsk_secret".chars() {
            app.update(
                Input::Key(KeyEvent {
                    code: KeyCode::Char(c),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                }),
                at(0),
            );
        }
        let mut terminal = render(&app, at(0));
        let screen = text(&terminal);
        assert!(screen.contains("Groq Whisper key › ••••••••••"), "{screen}");
        assert!(!screen.contains("gsk_secret"), "the key is never drawn");
        let (x, y) = find(&terminal, "••••••••••");
        terminal.backend_mut().assert_cursor_position((x + 10, y));
    }
}

mod orbit_view {
    use super::*;
    use crate::orbit::{Link, Ring};

    fn key(app: &mut App, code: KeyCode) {
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

    /// Cells drawn in Braille, which only the orbit uses.
    fn braille(terminal: &Terminal<TestBackend>) -> Vec<(u16, u16, Color)> {
        let buffer = terminal.backend().buffer();
        let mut cells = Vec::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol().chars().next().is_some_and(|c| ('\u{2801}'..='\u{28FF}').contains(&c)) {
                    cells.push((x, y, cell.fg));
                }
            }
        }
        cells
    }

    #[test]
    fn each_machine_is_a_ring_and_each_thread_a_bead_in_a_stable_order() {
        let app = fleet(110, 16);
        let fleet = orbit_fleet(&app);
        assert_eq!(
            fleet.rings,
            vec![
                Ring { link: Link::Down, beads: vec![] },
                Ring { link: Link::Live, beads: vec![AgentStatus::Blocked] },
                Ring { link: Link::Down, beads: vec![] },
            ],
            "local has no server, the studio is live, the laptop is lost"
        );
        // Beads go by thread id, not by the list's attention order, so a
        // status change never reshuffles them.
        let app = loaded(110, 30);
        let ids: Vec<&str> = app.threads.iter().map(|t| t.id.as_str()).collect();
        assert_ne!(
            ids,
            {
                let mut sorted = ids.clone();
                sorted.sort();
                sorted
            },
            "the list is not in id order"
        );
        let mut by_id: Vec<_> = app.threads.iter().map(|t| (t.id.clone(), t.status)).collect();
        by_id.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(orbit_fleet(&app).rings[0].beads, by_id.into_iter().map(|(_, s)| s).collect::<Vec<_>>());
    }

    #[test]
    fn a_connecting_machine_has_a_connecting_ring() {
        let app = App::new(110, 30, vec![MachineInfo { id: "studio".into(), label: "Mac Studio".into() }]);
        let rings = orbit_fleet(&app).rings;
        assert_eq!(rings.iter().map(|r| r.link).collect::<Vec<_>>(), vec![Link::Connecting, Link::Connecting]);
    }

    #[test]
    fn failed_launches_are_not_beads() {
        let mut app = loaded(110, 30);
        let mut failed = app.threads[0].clone();
        failed.id = format!("{LAUNCH_PREFIX}abc");
        app.threads.push(failed);
        assert_eq!(orbit_fleet(&app).rings[0].beads.len(), 4);
    }

    #[test]
    fn the_screen_animates_only_while_the_orbit_shows() {
        let mut app = App::new(110, 30, vec![]);
        app.update(snapshot(LOCAL, vec![]), at(0));
        assert!(app.open.is_none());
        assert!(app.animating(), "an empty session shows the orbit");
        let mut app = loaded(110, 30);
        assert!(app.open.is_some(), "the first thread opens on its own");
        assert!(!app.animating(), "an agent's terminal does not need frames");
        key(&mut app, KeyCode::Char('n'));
        assert!(app.animating(), "the composer shows the orbit");
        key(&mut app, KeyCode::Esc);
        assert!(!app.animating());
        let mut app = App::new(110, 30, vec![]);
        app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::NoServer }, at(0));
        assert!(app.needs_server_screen());
        assert!(!app.animating(), "the start-a-server screen has no orbit");
    }

    #[test]
    fn the_composer_draws_the_orbit_below_the_controls_in_theme_colours() {
        let mut app = loaded(110, 50);
        key(&mut app, KeyCode::Char('n'));
        let terminal = render(&app, at(1));
        let cells = braille(&terminal);
        assert!(!cells.is_empty(), "{}", text(&terminal));
        let palette = Palette::default();
        let screen = text(&terminal);
        let title = screen.lines().position(|l| l.contains("New thread  ")).expect("title");
        let prompt = screen.lines().position(|l| l.contains("What should we build?")).expect("prompt");
        let rows: Vec<usize> = cells.iter().map(|c| c.1 as usize).collect();
        let controls = screen.lines().position(|l| l.contains("⌃T dictate")).expect("controls");
        assert!(rows.iter().all(|&y| y > controls + 1), "decoration stays below the controls:\n{screen}");
        assert!(cells.iter().all(|&(x, _, _)| x > app.layout.terminal.x));
        assert_eq!(prompt - title, 2, "the task stays near the top at every height:\n{screen}");
        for line in ["Workspace", "⌃T dictate"] {
            assert!(screen.contains(line), "everything under the task still shows: {line}");
        }
        let colours: Vec<Color> = cells.iter().map(|c| c.2).collect();
        assert!(colours.contains(&palette.accent), "the core");
        assert!(colours.contains(&palette.surface1) && colours.contains(&palette.overlay0), "a ring's back and front");
        for status in [AgentStatus::Blocked, AgentStatus::Working, AgentStatus::Done] {
            let colour = status_color(status, &palette);
            // Beads move; over a few moments each shows.
            let seen = (0..12).any(|step| braille(&render(&app, at(1 + step * 5))).iter().any(|c| c.2 == colour));
            assert!(seen, "a {status:?} bead");
        }
    }

    #[test]
    fn the_composer_with_room_to_spare() {
        let mut app = loaded(100, 44);
        key(&mut app, KeyCode::Char('n'));
        insta::assert_snapshot!(text(&render(&app, at(7))));
    }

    #[test]
    fn the_task_box_and_cursor_stay_at_the_same_height_with_or_without_an_orbit() {
        let mut app = loaded(100, 44);
        key(&mut app, KeyCode::Char('n'));
        key(&mut app, KeyCode::Char('h'));
        let mut terminal = render(&app, at(1));
        let screen = text(&terminal);
        let task = screen.lines().position(|l| l.contains("│ h")).expect("task line");
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!(task, 5, "the task does not move down for decoration");
        assert_eq!(cursor.y as usize, task, "{screen}");
        assert_eq!(cursor.x, app.layout.terminal.x + 5);
        // Without spare room the input sits at exactly the same height.
        let mut app = loaded(100, 24);
        key(&mut app, KeyCode::Char('n'));
        let screen = text(&render(&app, at(1)));
        assert_eq!(screen.lines().position(|l| l.contains("What should we build?")), Some(3), "{screen}");
    }

    #[test]
    fn a_picker_opens_under_its_field() {
        let mut app = loaded(100, 44);
        key(&mut app, KeyCode::Char('n'));
        key(&mut app, KeyCode::F(3));
        let screen = text(&render(&app, at(1)));
        let field = screen.lines().position(|l| l.contains("Harness ")).expect("field");
        let popup = screen.lines().position(|l| l.contains("╭ Harness ")).expect("popup");
        assert_eq!(popup, field + 1, "{screen}");
    }

    #[test]
    fn the_orbit_moves_with_the_clock() {
        let mut app = loaded(110, 50);
        key(&mut app, KeyCode::Char('n'));
        let a = text(&render(&app, at(1)));
        let b = text(&render(&app, SystemTime::UNIX_EPOCH + Duration::from_millis(1_100)));
        assert_ne!(a, b, "a tenth of a second later the frame differs");
        assert_eq!(a, text(&render(&app, at(61))), "and a minute later it is the same again");
    }

    #[test]
    fn without_room_there_is_no_orbit() {
        let mut app = loaded(110, 24);
        key(&mut app, KeyCode::Char('n'));
        assert!(braille(&render(&app, at(1))).is_empty(), "the fields fill a 24-row window");
        let mut app = App::new(34, 30, vec![]);
        app.update(snapshot(LOCAL, vec![]), at(0));
        assert!(app.layout.terminal.width < orbit::MIN_ROWS * 2);
        assert!(braille(&render(&app, at(1))).is_empty(), "too narrow beside the list");
    }

    #[test]
    fn an_open_picker_covers_the_orbit() {
        let mut app = loaded(110, 50);
        key(&mut app, KeyCode::Char('n'));
        let before = braille(&render(&app, at(1))).len();
        key(&mut app, KeyCode::F(3));
        assert!(app.composer.picker.is_some());
        let terminal = render(&app, at(1));
        assert!(braille(&terminal).len() <= before, "{}", text(&terminal));
    }
}

mod self_updates {
    use super::*;
    use crate::app::{UpdatePhase, Updates};
    use crate::update::{Installed, Release};

    fn release() -> Release {
        Release {
            tag: "v2.0.0".into(),
            version: semver::Version::new(2, 0, 0),
            notes: "## Changes\nFaster inbox navigation.\nSafer updates.".into(),
            newer: true,
            asset: Some("herdr-inbox-linux-x86_64.tar.gz".into()),
        }
    }

    fn showing(phase: UpdatePhase) -> App {
        let mut app = loaded(100, 30);
        app.updates = Updates { visible: true, phase, scroll: 0 };
        app
    }

    #[test]
    fn the_dialog_shows_the_versions_notes_and_confirmation_keys() {
        let app = showing(UpdatePhase::Ready(release()));
        let terminal = render(&app, at(2));
        let screen = text(&terminal);
        assert!(screen.contains(&format!("Running {}", crate::update::CURRENT_VERSION)), "{screen}");
        for needle in [
            "Latest stable 2.0.0",
            "Enter installs this release",
            "Faster inbox navigation.",
            "Enter install",
            "b GitHub",
            "Esc back",
        ] {
            assert!(screen.contains(needle), "missing {needle:?}:\n{screen}");
        }
        // Assert the real package version above, but keep the golden layout
        // independent of Release Please's version bumps and their text width.
        let running = format!("Running {}", crate::update::CURRENT_VERSION);
        let screen = screen
            .lines()
            .map(|line| match line.split_once(&running) {
                Some((before, after)) => {
                    let width = running.len() + after.chars().take_while(|ch| *ch == ' ').count();
                    format!("{before}{:<width$}{}", "Running <version>", after.trim_start_matches(' '))
                }
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn checking_installing_errors_and_restart_instructions_stay_visible() {
        for (phase, needle) in [
            (UpdatePhase::Checking, "Checking GitHub"),
            (UpdatePhase::Installing(release()), "Downloading and checking SHA-256"),
            (UpdatePhase::Failed { release: None, error: "HTTP 403: rate limit".into() }, "HTTP 403: rate limit"),
            (UpdatePhase::Failed { release: Some(release()), error: "checksum mismatch".into() }, "checksum mismatch"),
            (
                UpdatePhase::Installed(Installed {
                    version: release().version,
                    path: "/custom/bin/herdr-inbox".into(),
                }),
                "/custom/bin/herdr-inbox",
            ),
        ] {
            let app = showing(phase);
            let terminal = render(&app, at(2));
            let screen = text(&terminal);
            assert!(screen.contains(needle), "{screen}");
            if matches!(app.updates.phase, UpdatePhase::Installed(_)) {
                assert!(screen.contains("Restart Inbox") && screen.contains("agents keep running"), "{screen}");
            }
        }
    }

    #[test]
    fn up_to_date_and_unavailable_releases_do_not_offer_an_install() {
        for (release, reason) in [
            (Release { newer: false, ..release() }, "No newer stable release"),
            (Release { asset: None, ..release() }, "no binary and checksum"),
        ] {
            let screen = text(&render(&showing(UpdatePhase::Ready(release)), at(2)));
            assert!(screen.contains(reason), "{screen}");
            assert!(!screen.contains("Enter install"), "{screen}");
            assert!(screen.contains("b GitHub"), "{screen}");
        }
    }

    #[test]
    fn notes_scroll_and_tiny_terminals_do_not_hide_or_crash_the_dialog() {
        let release = Release { notes: (0..80).map(|i| format!("Change {i}\n")).collect(), ..release() };
        let mut app = showing(UpdatePhase::Ready(release));
        app.updates.scroll = 70;
        let screen = text(&render(&app, at(2)));
        assert!(screen.contains("Change 70") && !screen.contains("Change 0\n"), "{screen}");
        for (width, height) in [(1, 1), (18, 6), (44, 16), (80, 24), (180, 60)] {
            app.update(Input::Resize { width, height }, at(2));
            render(&app, at(2));
        }
    }

    #[test]
    fn the_update_shortcut_is_advertised_only_where_it_is_handled() {
        let mut app = loaded(120, 30);
        for focus in [Focus::List, Focus::Composer] {
            app.focus = focus;
            assert!(text(&render(&app, at(1))).contains("⌃G updates"));
        }
        app.focus = Focus::Terminal;
        assert!(!text(&render(&app, at(1))).contains("⌃G updates"));
        app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::NoServer }, at(2));
        assert!(text(&render(&app, at(2))).contains("⌃G updates"));
    }
}

mod space_hold_hints {
    use super::*;
    use crate::app::{Dictation, Phase, Target};
    use crate::speech::backends::SpeechConfig;

    fn recording(held: bool) -> Dictation {
        Dictation {
            target: Target::Composer,
            phase: Phase::Recording,
            levels: vec![],
            quiet: false,
            started: at(0),
            then: None,
            held,
        }
    }

    fn bar(app: &App) -> String {
        text(&render(app, at(3))).lines().last().unwrap().to_string()
    }

    #[test]
    fn a_held_recording_says_letting_go_types() {
        let mut app = loaded(110, 30);
        app.dictation = Some(recording(true));
        let line = bar(&app);
        assert!(line.contains("release type  ↵ send  esc discard"), "{line}");
        assert!(!line.contains("⌃T"), "{line}");
        app.dictation = Some(recording(false));
        assert!(bar(&app).contains("↵ send  ⌃T type  esc discard"), "Ctrl+T dictation keeps its keys");
    }

    #[test]
    fn the_agent_and_composer_show_ctrl_t_unless_hold_is_enabled() {
        for space_hold in [None, Some(false), Some(true)] {
            let mut app = loaded(110, 30);
            app.config.speech = SpeechConfig { space_hold, ..SpeechConfig::default() };
            let (hint, absent) = if space_hold == Some(true) {
                ("hold ␣ dictate", "⌃T dictate")
            } else {
                ("⌃T dictate", "hold ␣")
            };
            app.focus = crate::app::Focus::Terminal;
            let line = bar(&app);
            assert!(line.contains(hint) && !line.contains(absent), "{line}");
            app.focus = crate::app::Focus::Composer;
            let screen = text(&render(&app, at(3)));
            assert!(screen.contains(hint) && !screen.contains(absent), "{screen}");
        }
    }
}
