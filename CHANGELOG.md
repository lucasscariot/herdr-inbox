# Changelog

All notable changes to Herdr Inbox. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [1.0.0] - Unreleased

Herdr Inbox is now one program, `herdr-inbox`, instead of a Herdr plugin plus
a patched Herdr client. It runs beside plain `herdr` on the same server and
never changes it.

### Added

- `herdr-inbox`, a standalone Rust client for Herdr. A live thread list grouped
  by what each agent needs, the selected agent's terminal next to it, fully
  interactive, and archiving. It talks to the running Herdr server through the
  JSON API and `herdr terminal session control` only, and follows the Herdr
  theme. Plain `herdr` keeps working beside it.
- Threads from every enabled saved SSH machine join the same list, open live
  over SSH, and can be archived. A status bar strip shows each machine's
  connection; an unreachable machine never blocks the others.
- The composer (`n`): write a task, pick project, machine, harness, model,
  thinking level and workspace, send. Launches run in the background into a
  new git worktree by default, with progress under Launches. An agent stopped
  at a startup dialog gets its task once the dialog is answered.
- `config.toml` for roots, branch prefix, harness arguments and more; the
  legacy plugin's `config.json` is read until it exists. `--config` reads
  another file.
- Dictation everywhere with `Ctrl+T`: the composer's task, a reply, or any
  agent, with a live meter. Enter sends, Ctrl+T types, Esc discards. `F10`
  connects Groq, Gemini, OpenAI, Mistral or Deepgram, or builds whisper.cpp.
- Optional hold-to-dictate: set `space_hold = true` under `[speech]`, hold
  Space to talk, and let go to type the words into the composer, a reply or
  any agent. A held bar is told apart by keyboard auto-repeat, so it works
  in every terminal and over SSH.
- Presets (`F7`, `Ctrl+D`), task history (`Ctrl+P`/`Ctrl+N`), a list filter
  (`/`), replies without opening a thread (`r`), sending a thread's task again
  (`e`), and failed launches as rows to retry or dismiss (`d`).
- Screenshots in the composer and in replies: `Ctrl+V`, or the desktop's
  paste when only an image is copied, adds it as `[Image #1]`. Claude Code and
  Codex receive it as an attachment, other agents as a path. This machine
  only for now.
- The plugin's remembered choices, presets, history and dictation keys are
  copied on first start.
- The orbit, the inbox's logo: a turning armillary sphere drawn in Braille
  above the composer's task and when no thread is open. Each ring is a
  machine and each bead a thread in its status colour; the core breathes
  while a thread needs input. It animates only while on screen.
- Prebuilt binaries for Linux and macOS on every release, and a one-line
  installer that checks their SHA-256.
- A quieter frame: the sidebar divider and an unfocused task box are faint
  hairlines, the focused task box and the reply box wear a softened accent,
  and idle threads (badge, marker, heading, orbit bead) are faint. Themes that
  map these tones to plain ANSI white or grey no longer draw white bars.

### Changed

- Ordinary spaces type immediately by default; Ctrl+T remains the dictation
  shortcut. Hold-to-dictate now needs an explicit `space_hold = true`.
- Forwarded agent keys no longer redraw the unchanged screen before their
  echo arrives, avoiding an extra 16 ms wait. The frame limit for streaming
  output stays in place. Runtime scheduling and PTY latency tests cover both.

- Moving through tasks with j/k, arrow keys, g/G or Home/End shows each
  discussion immediately, without Enter. Keyboard focus stays in the list;
  Enter or Tab moves it to the agent. Finished tasks stay ready while browsing
  so their rows do not move until the agent gets keyboard focus.

### Removed

- The Herdr plugin (composer and inbox panes) and the patched thread-only
  Herdr client: `herdr-inbox` replaces both. See "Coming from the plugin" in
  the README to move over.

## [0.5.0] - 2026-10-08

### Added

- `dictate` action and `python3 main.py dictate`: record a follow-up for any
  running thread, then Enter submits it through Herdr's prompt transport,
  Ctrl+T types it unsent, and Esc discards it. Bind the action to `ctrl+t` for
  the stock client; the thread-only client handles Ctrl+T everywhere, including
  threads on saved machines.
- Thread-only client: Tab moves between the thread list and the terminal, j/k
  walk the threads, Enter opens, Backspace archives, and a `THREADS` mode bar
  shows the keys. Shift+Tab and the composer's own Tab are untouched.
- Thread-only client: archive a thread from its sidebar row. Right-click any
  row for "Archive thread", or click "archive" on the thread that is open. It
  closes that thread's workspace on whichever machine owns it, keeps a linked
  worktree checkout on disk, and honours Herdr's `ui.confirm_close`.
- Thread-only client: the sidebar groups threads under "needs input",
  "ready", "working", and "idle" headings when sorted by priority, with a
  count per group.

### Changed

- Thread-only client: rows carry a status rail and badge (Input, Ready,
  Working, Idle) instead of a uniform "Waiting"; idle threads are muted so the
  ones that need a look stand out. Each row is three lines: project, task,
  then branch with the harness and, for threads off Local, the machine.
- Thread-only client: the New thread control is a filled button with its key
  binding; the sidebar needs one fewer line per thread.

## [0.4.0] - 2026-10-08

First public release.

### Added

- Worktree-first launching: run a thread in the main checkout, an existing
  worktree, or a new worktree whose branch is named after the task.
- Discovery groups linked worktrees under their repository and reports each
  checkout's branch.
- Non-blocking sends with a launch strip under the composer; Ctrl+Enter sends
  and keeps the draft; Ctrl+P recalls task history.
- Live inbox fed by Herdr socket events on every machine, with per-machine link
  state, replies from the inbox, edit-and-relaunch, filtering, and dismissal of
  failed launches.
- Launches that stop at a startup dialog keep their task and send it once the
  agent is idle.
- Dictation with Ctrl+T, a guided Dictation menu (F10) for local transcribers,
  hosted services with verified keys, a whisper.cpp build, or a custom command.
- Detection of every Herdr-supported agent kind; model catalogs cached between
  sessions.
- A Braille wordmark lit like the knot, and a knot that scales with the terminal.

### Changed

- "Favorites" and "combos" are now "presets" everywhere.
- Fuzzy ranking in every picker; word-aware wrapping in the task box.
- Remote SSH calls share one control socket per machine.

[0.5.0]: https://github.com/lucasscariot/herdr-inbox/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/lucasscariot/herdr-inbox/releases/tag/v0.4.0
