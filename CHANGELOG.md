# Changelog

All notable changes to Herdr Inbox. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [1.5.0](https://github.com/lucasscariot/herdr-inbox/compare/v1.4.0...v1.5.0) (2026-10-16)


### Features

* **api:** launch tasks sent from a web page over tailscale ([#49](https://github.com/lucasscariot/herdr-inbox/issues/49)) ([3e2f811](https://github.com/lucasscariot/herdr-inbox/commit/3e2f8112870bce42d889036906ec894d797e05f9))
* **composer:** name installed skills with $ in tasks and replies ([#47](https://github.com/lucasscariot/herdr-inbox/issues/47)) ([9293e4d](https://github.com/lucasscariot/herdr-inbox/commit/9293e4d14b753cf4e4e96ab8848303892fe3d014))

## [1.4.0](https://github.com/lucasscariot/herdr-inbox/compare/v1.3.0...v1.4.0) (2026-10-11)


### Features

* **composer:** pick the harness with the most usage left ([#44](https://github.com/lucasscariot/herdr-inbox/issues/44)) ([02c317d](https://github.com/lucasscariot/herdr-inbox/commit/02c317da8f39f91a79315baf1af02b8271a5146b))
* **composer:** reorder presets with page up and page down ([#45](https://github.com/lucasscariot/herdr-inbox/issues/45)) ([f7f3f76](https://github.com/lucasscariot/herdr-inbox/commit/f7f3f76f5149a30c2c9a8e91feffa9bbe4356d73))


### Bug Fixes

* **composer:** keep the first-preset default pending until discovery answers ([#35](https://github.com/lucasscariot/herdr-inbox/issues/35)) ([e232ae2](https://github.com/lucasscariot/herdr-inbox/commit/e232ae23fc7de54f702b0fdf96cb4cced12d2b0e))
* **launch:** let trailing filler close a sentence for a particle ([#40](https://github.com/lucasscariot/herdr-inbox/issues/40)) ([ddf2ac6](https://github.com/lucasscariot/herdr-inbox/commit/ddf2ac625bd24aedd3fc8d1161216ff2db2b600a))
* **launch:** retry a failed launch in its worktree without losing the task ([#42](https://github.com/lucasscariot/herdr-inbox/issues/42)) ([217651e](https://github.com/lucasscariot/herdr-inbox/commit/217651eb48c254da082a79c0d1f57123da16e069))
* **launch:** sharpen branch names on dates, subjects and abbreviations ([#36](https://github.com/lucasscariot/herdr-inbox/issues/36)) ([f1b3c7c](https://github.com/lucasscariot/herdr-inbox/commit/f1b3c7cee35e3e90d9a4c5f08937f474c6bbfa03))
* **launch:** start OpenCode 2 on the chosen model through a session ([#43](https://github.com/lucasscariot/herdr-inbox/issues/43)) ([859707c](https://github.com/lucasscariot/herdr-inbox/commit/859707c44cfb7b2ae5e233cda89e548d125ed0fa))
* **launch:** stop a bare "to" from marking the next word as a verb ([#38](https://github.com/lucasscariot/herdr-inbox/issues/38)) ([2215b8e](https://github.com/lucasscariot/herdr-inbox/commit/2215b8e28bb377f4963b7c80990ead23c2444d07))
* **ui:** keep the highlighted thread readable on ANSI themes ([#41](https://github.com/lucasscariot/herdr-inbox/issues/41)) ([5443efd](https://github.com/lucasscariot/herdr-inbox/commit/5443efd50f4e0f45a08c6999181d0df0c70cce0c))
* **ui:** reserve theme accents for focus and selection ([#39](https://github.com/lucasscariot/herdr-inbox/issues/39)) ([aa6e7c9](https://github.com/lucasscariot/herdr-inbox/commit/aa6e7c97d55ef7210752d7ccc7c250f642f1af44))

## [1.3.0](https://github.com/lucasscariot/herdr-inbox/compare/v1.2.1...v1.3.0) (2026-10-10)


### Features

* **composer:** show presets as a strip and start new threads on the first one ([#31](https://github.com/lucasscariot/herdr-inbox/issues/31)) ([74cc19c](https://github.com/lucasscariot/herdr-inbox/commit/74cc19cba61939fc774bfa9bd80744173e644426))
* **launch:** name branches from the content words of the first sentence ([#32](https://github.com/lucasscariot/herdr-inbox/issues/32)) ([0fba2ee](https://github.com/lucasscariot/herdr-inbox/commit/0fba2ee1d44a008974b410e4d796810b7d03370c))
* **threads:** archive with a double tap and cancel with esc ([#30](https://github.com/lucasscariot/herdr-inbox/issues/30)) ([c5eb190](https://github.com/lucasscariot/herdr-inbox/commit/c5eb190c9f3dfca940eca775a80983e4939ffef5))
* **ui:** split the sidebar header into stats, actions and list ([#33](https://github.com/lucasscariot/herdr-inbox/issues/33)) ([efec082](https://github.com/lucasscariot/herdr-inbox/commit/efec0823ab217087a90a6f1b58952af64716f818))

## [1.2.1](https://github.com/lucasscariot/herdr-inbox/compare/v1.2.0...v1.2.1) (2026-10-10)


### Bug Fixes

* **ci:** keep a hung end-to-end test from holding the macOS runner ([#28](https://github.com/lucasscariot/herdr-inbox/issues/28)) ([12ab932](https://github.com/lucasscariot/herdr-inbox/commit/12ab9328f0831ff34f75baa55539d721bd829e97))

## [1.2.0](https://github.com/lucasscariot/herdr-inbox/compare/v1.1.1...v1.2.0) (2026-10-10)


### Features

* **composer:** compare a task across two or three agents ([#26](https://github.com/lucasscariot/herdr-inbox/issues/26)) ([213c291](https://github.com/lucasscariot/herdr-inbox/commit/213c291006aca627744240aee8ae406b61f3b2b9))

## [1.1.1](https://github.com/lucasscariot/herdr-inbox/compare/v1.1.0...v1.1.1) (2026-10-10)


### Bug Fixes

* **ui:** replace the pixel harness marks with one glyph per harness ([#24](https://github.com/lucasscariot/herdr-inbox/issues/24)) ([6d2986d](https://github.com/lucasscariot/herdr-inbox/commit/6d2986d8c6df6f7ae3e2d288094ff7f391c47870))

## [1.1.0](https://github.com/lucasscariot/herdr-inbox/compare/v1.0.0...v1.1.0) (2026-10-10)


### Features

* **ui:** add clickable composer controls and harness marks ([#21](https://github.com/lucasscariot/herdr-inbox/issues/21)) ([7b159a4](https://github.com/lucasscariot/herdr-inbox/commit/7b159a4572a80f7fe574899fdb678c6f0e0f2bc0))


### Bug Fixes

* **ci:** stabilize checks and run each one once ([#23](https://github.com/lucasscariot/herdr-inbox/issues/23)) ([e65500f](https://github.com/lucasscariot/herdr-inbox/commit/e65500fb90d949f6406f9155793dcd3f5dabda4b))

## [Unreleased]

### Documentation

- Rework the README with example inbox and composer screenshots, a clearer
  product pitch, a first-task walkthrough and links to star the project.
  State the Python 3.9+ discovery requirement for local and SSH machines.

### Added

- Type `$` in the composer or a reply to pick a skill installed on the
  target machine, from `~/.agents/skills` and each harness's own directory
  (Codex, Claude Code, OpenCode, Pi, Gemini, Copilot). The task keeps
  `$name` as written, and the agent also gets each mentioned skill's
  `SKILL.md` path, so any harness can use any of them.
- A **Usage** row under the composer's choices shows each subscription's
  windows (`✻ Claude 5h 38% · week 29%  ·  ◆ Codex week 17%`), yellow past
  80 %, red when a limit is reached, faint with their age once stale. The
  Harness list starts with **Auto · most usage left**, which picks the
  installed harness with the most headroom before any limit, follows the
  numbers, keeps the model last used with that harness, and is remembered
  per project. Codex is asked through its own app server; Claude Code
  reports through a status line wrapper that every Claude thread the inbox
  launches gets, chained with the user's own status line. `[usage]` in the
  config turns it off or maps other CLIs to a provider.
- `PageUp` and `PageDown` reorder the highlighted preset one place earlier
  or later on the strip, saving immediately and keeping it highlighted.
  The strip's key hint shows these shortcuts; `Ctrl+←`/`Ctrl+→` still work.
- A preset strip right under the task: every preset is a chip in the user's
  order of preference, the chip in use is highlighted, and a "+ Save as
  preset…" chip (or `Ctrl+D`) saves the current choices. `F7` focuses the
  strip, `←`/`→` move, `Enter` applies, `1`-`9` apply the nth preset,
  `Ctrl+←`/`Ctrl+→` reorder and persist the order, `Ctrl+R` renames,
  `Delete` removes, and chips are clickable.
- A new thread starts on the first preset whose CLI is installed on the
  chosen machine; the composer goes back to it after each send. Send and
  keep (`Ctrl+S`) and resending a thread keep their own choices.
- Compare agents from the composer (`F12`, off by default): the same task
  goes to two or three agents at once, each in its own new worktree whose
  branch ends with the agent's harness and model. One bad choice launches
  nothing, the comparison resets after sending, and only the first agent's
  choices are remembered.
- Clickable composer text, fields, picker choices and send action, with mouse
  wheel navigation in pickers. The input stays near the top of a tall window.
- Coloured harness glyphs for Claude Code, Codex, Pi and OpenCode, readable
  harness names before branches, a clickable sidebar filter, a position
  counter, PageUp/PageDown navigation and a scroll thumb.
- Codex thinking selection using each model's advertised reasoning levels and
  the CLI's config override. Model changes drop incompatible levels; old model
  caches refresh once to pick up the new capabilities.

### Changed

- The sidebar's five-column pixel marks are replaced by one small glyph per
  harness on the metadata line, so thread rows get their full width back.
- Archiving is a double tap: pressing `Backspace`, `Delete` or `x` again
  confirms, alongside `Enter` and `y`, and `Esc` cancels the prompt.
- The sidebar header has three bands divided by hairline rules: the wordmark
  with a count per status (taken before the filter, so filtering never hides
  what is waiting), then New thread and the filter field, then the list. The
  filter shows `N of M` while it is active.

### Fixed

- OpenCode 2 starts on the chosen model again. It refuses `--model` on its
  interactive command, so the inbox now creates an OpenCode session with
  the model (`opencode api session.create`, on the `--server` in its
  arguments if any) and starts OpenCode with `--session`. A launch that
  cannot work is refused before anything is created: a model that is not
  `provider/model`, `--standalone`, a disabled OpenCode background service,
  or `--continue`, `--fork` and `--session` in OpenCode's arguments. If
  OpenCode then fails to start, its session is deleted.
- Retrying a failed launch (`e`) no longer fails with "already exists" when
  the launch had made its worktree: the retry reuses that worktree, keeps the
  task, can use any preset, and retires the failed launch once sent. Failed
  rows show `e retry`, the failure notice says how to retry, and dismissing
  one points to the task in the composer history (`Ctrl+P`).
- Read the theme from `HERDR_CONFIG_PATH` when set, matching Herdr instead of
  silently using the default config file.
- Limit Herdr's configured accent to focus and selected items rather than
  headings, inactive actions and mode labels. Hints and branch metadata use
  neutral theme colours; the previous discussion loses its accent while the
  composer is open. Status colours, harness glyphs and agent output stay intact.
- Herdr E2E sandboxes wait for an API ping instead of treating a socket file as
  server readiness, and report startup exits with their stderr.
- Release version bumps no longer break the update dialog's layout snapshot;
  its displayed compiled version is still asserted separately.
- Description-only PR edits no longer use CI runners, cancel running code
  checks or replace a pending run.
- Each check runs once per push: the release workflow no longer repeats the CI
  suite on PR heads, the release PR is tested by CI alone rather than by a full
  build, and a newer PR head cancels obsolete release builds.
- Sidebar wheel scrolling no longer snaps back to the selection on every tick.
- Model ids remain selectable when their display name differs from the id.
- Composer backgrounds, popup titles and dialog frames now use explicit theme
  styles rather than terminal-default white. Agent output colours stay intact.
- Linux release builds use native Rust cross-compilation and LLVM linking,
  avoiding the emulated GCC linker crash. Verify the ELF architecture before
  exporting each binary.

## [1.0.0](https://github.com/lucasscariot/herdr-inbox/compare/v0.4.0...v1.0.0) (2026-10-10)


### ⚠ BREAKING CHANGES

* automate standalone releases on self-hosted runners ([#18](https://github.com/lucasscariot/herdr-inbox/issues/18))

### Features

* **composer:** paste screenshots into the composer and replies ([#13](https://github.com/lucasscariot/herdr-inbox/issues/13)) ([b46e836](https://github.com/lucasscariot/herdr-inbox/commit/b46e836ff8983ba5b7870f179ff61ff55d2c130d))
* **install:** add ctrl+g self-updates ([#17](https://github.com/lucasscariot/herdr-inbox/issues/17)) ([6e26ef0](https://github.com/lucasscariot/herdr-inbox/commit/6e26ef040141d4b2e30630e6647e5df6e2368772))


### Bug Fixes

* **threads:** show discussions while navigating tasks ([#15](https://github.com/lucasscariot/herdr-inbox/issues/15)) ([1dd8542](https://github.com/lucasscariot/herdr-inbox/commit/1dd85425fd7c67553e7b6aa2175b06a893a07d35))
* **ui:** soften the sidebar divider, the task frame and idle tones ([#14](https://github.com/lucasscariot/herdr-inbox/issues/14)) ([6c07768](https://github.com/lucasscariot/herdr-inbox/commit/6c07768b1a9ad3d34c5aeb4584d287af86583eb0))


### Performance Improvements

* **ui:** remove typing delays and add latency regression tests ([#16](https://github.com/lucasscariot/herdr-inbox/issues/16)) ([75928ef](https://github.com/lucasscariot/herdr-inbox/commit/75928ef935f888d9a9f71553cc1aa631c732b73d))


### Continuous Integration

* automate standalone releases on self-hosted runners ([#18](https://github.com/lucasscariot/herdr-inbox/issues/18)) ([40104af](https://github.com/lucasscariot/herdr-inbox/commit/40104afea0c77753d4c13dc510056ab296dfa49e))

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
- Release Please maintains Rust version bumps and release notes. CI and
  four-platform release builds use the self-hosted Blueprint Linux runners
  and a dedicated macOS runner. Releases stay draft until tests and all
  binaries and checksums pass; fork PRs never run on the personal runners.
- Ctrl+G in the thread list or composer checks GitHub's latest stable release,
  shows its notes and opens its page with B. Enter confirms installation of
  that exact release into the running binary's directory, after verifying
  its SHA-256 and version. Checks and installs run in the background, drafts
  stay intact, and a successful update asks for a restart without changing
  Herdr or stopping agents.
- A quieter frame: the sidebar divider and an unfocused task box are faint
  hairlines, the focused task box and the reply box wear a softened accent,
  and idle threads (badge, marker, heading, orbit bead) are faint. Themes that
  map these tones to plain ANSI white or grey no longer draw white bars.

### Changed

- Subprocess tests wait for their fixtures to become ready before release or
  SIGINT, avoiding startup races on busy self-hosted macOS runners.
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
