# Changelog

All notable changes to Herdr Inbox. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

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

[0.4.0]: https://github.com/lucasscariot/herdr-inbox/releases/tag/v0.4.0
