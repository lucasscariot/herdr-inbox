# Architecture

Herdr Inbox is a single Rust binary. It owns no agent state: everything it
shows comes from a Herdr server, and everything it does goes through Herdr's
public interfaces.

```
 crossterm events ──┐
 link (Herdr API) ──┼──► App::update(input) ──► effects ──► runtime
 terminal session ──┘          │                              │
                               ▼                              ├─ herdr terminal session control
                          ui::draw(&App)                      ├─ workspace.close
                                                              └─ herdr server
```

## Modules

| Module | Role |
| --- | --- |
| `herdr::socket` | Resolves the server's socket like the `herdr` CLI: `--session`, `HERDR_SOCKET_PATH`, `HERDR_SESSION`, default. |
| `herdr::api` | JSON socket API. One request per connection, because Herdr answers the first line and closes. |
| `herdr::events` | `events.subscribe` streams. Each subscription owns its connection; Herdr resets one that receives anything else. |
| `herdr::terminal` | Runs `herdr terminal session control <pane> --takeover` and speaks its JSON lines: base64 ANSI frames out, input/resize/scroll/mouse/release in. |
| `herdr::ssh` | Builds SSH commands for a saved machine: one ControlMaster connection per host, `sh -lc` with the usual install directories on PATH, a ready marker before `exec herdr` so login noise is skipped, single-quoted arguments that bash, zsh and fish read alike. |
| `herdr::transport` | One machine's API calls, subscriptions, `herdr` runner and git probe. Local goes to the socket; remote runs `herdr remote-api-bridge` over SSH, one process per call and one per subscription. |
| `machines` | Reads enabled saved machines from `herdr machine list --json`. |
| `link` | One per machine. Keeps that server in sync: ping, one lifecycle subscription, one status subscription for all agent panes (replaced when the pane set changes), debounced snapshots with the git facts of their checkouts (cached 15 s), a 30 s resync, reconnection with backoff. |
| `config` | `config.toml` (or the legacy plugin's `config.json`): roots, depth, branch prefix, harness arguments and executables, extra models, per-machine overrides. |
| `state` | Remembered choices, cached inventories and launch journals under `$XDG_STATE_HOME/herdr-inbox`, written atomically and privately. |
| `discovery` | An embedded Python 3.9+ probe, run with `python3` on each machine (over SSH through a login shell), finds projects (with the GitHub `owner/name` of their `origin`), worktrees, installed agent CLIs and their model and thinking catalogs, and the user's skills (`~/.agents/skills` first, then each harness's own directory; a name is kept once and skills are read on every pass). Codex's cache supplies per-model reasoning levels, with `CODEX_HOME` respected. Models are cached 15 minutes; a capability revision expires older inventories once. |
| `usage` | Subscription usage per provider (Anthropic behind `claude`, OpenAI behind `codex`, or as the config maps a harness), and the pick of the harness with the most headroom. An embedded Python probe asks Codex's own app server (`account/rateLimits/read`, session log as fallback) and reads the cache a status line wrapper, also embedded and installed by the probe, fills from the JSON Claude Code hands its status line. Reports are cached per machine; a project can remember `auto` instead of a harness. |
| `launch` | Plans a launch (branch name, flags) without side effects, then runs it step by step with the `herdr` CLI: worktree or workspace, tab title, `agent start`, thread metadata, `agent prompt`. A journal is written before the first change and after each step. A comparison is one plan per agent, each with its own worktree, planned together before any runs. |
| `editor`, `fuzzy` | The composer's text box, cell-to-cursor placement using the same Unicode-aware wrapping as drawing, and its pickers' ranking. Model pickers match labels and ids. |
| `skills` | `$skill` mentions. Pure. The menu follows the editor (a `$` word at the cursor); on send, the text stays as written and a footer lists each mentioned skill's `SKILL.md`, so any harness can read it. |
| `images` | Pasted images: `[Image #N]` placeholders in the editor, their paths in the sent text, and the split that pastes each image on its own so agents attach it. Pure. |
| `clipboard` | Reads the clipboard for `Ctrl+V` (`wl-paste`, `xclip`, `osascript`) and saves an image privately under the cache directory. |
| `presets` | Named harness, model and thinking combinations, in the plugin's `presets.json` format, kept in the user's order of preference. |
| `speech` | Dictation: `recorder` (whatever is installed, stopped with SIGINT so the WAV is finalized), `meter` (an FFT over the file being written, twelve bands, an adaptive noise floor), `backends` (a command, voxtype, whisper.cpp, or a hosted service through `curl` with the key on stdin), `whisper` (builds whisper.cpp and downloads a model). |
| `update` | Checks GitHub's latest stable release through a bounded `curl` request, compares semantic versions, and runs the embedded checksum-checked installer against the current executable. |
| `app::updates` | The update dialog's state, confirmation and notes scrolling. Checks and installs are effects, never I/O in the app. |
| `app` | The state machine. `update(Input) -> Vec<Effect>`; no I/O, so every behaviour is unit-tested. |
| `app::composer::layout` | Pure geometry shared by composer drawing and mouse input: task viewport, the preset strip's wrapping chips, field rows, send target and scrolled picker rows. Decoration sits below the controls. |
| `threads` | Turns a snapshot into labelled, grouped, sorted threads; tracks when statuses changed and which finished threads the user has seen. |
| `screen` | A vt100 emulator fed with Herdr's frames, drawn into ratatui cells. |
| `keys` | Encodes key presses as xterm bytes for the pane, honouring its cursor-key and bracketed-paste modes. |
| `git` | Reads repository name and branch from `.git` files, without running git. |
| `hold` | Telling a held space bar from typed spaces by key-repeat timing. Pure. |
| `orbit` | The orbit logo: the fleet as rings and beads, rendered to Braille cells. Pure, a function of the time. |
| `theme` | Herdr's built-in palettes and `config.toml` overrides. |
| `ui` | Pure drawing from `&App`, including themed frames and explicit popup title styles. |
| `ui::harness` | One single-width coloured glyph per harness, drawn before its readable name in the sidebar, the preset chips and the composer picker. Claude, Codex, Pi and OpenCode have distinct glyphs; other harnesses get a neutral one. |
| `api` | Opt-in prompts from one web page over HTTP on 127.0.0.1, published by `tailscale serve`. The routing is pure: one path, the configured origin (CORS preflight included) and Tailscale login, JSON. An accepted prompt becomes `Input::ApiPrompt`; the request waits for the app's `Effect::ApiReply`. |
| `app::api` | Launches an API prompt from a fresh composer through `App::send`, so the harness (Auto), model, branch, history and launch row are the composer's own; the composer on screen is restored untouched. The project is the local one whose `origin` is the repository on GitHub. |
| `runtime` | Terminal setup, the event loop, effect execution. |
| `runtime::redraw` | Applies inputs and schedules draws with a 16 ms frame limit. Forwarded keys wait for the agent's echo instead of redrawing an unchanged screen. |

## Build and release

Release Please's Rust strategy updates the crate, lockfile, changelog and
release manifest in a reviewed PR. Merging it creates a tagged draft release.
The same workflow tests that commit, builds four archives, verifies their
checksums and publishes the draft only after every job succeeds. Keeping
publication in one workflow avoids GitHub's suppression of tag events created
by `GITHUB_TOKEN`.

Linux jobs run in the Blueprint's ARM64 Docker runners on Mac Studio. BuildKit
cross-compiles static musl binaries from a narrow source context using a native
Rust compiler and `rust-lld` for the final link. A target-platform stage checks
the ELF architecture and version, so only the brief x86_64 execution uses
emulation. macOS jobs use a dedicated native ARM64 runner and Xcode, with
Rosetta to check the x86_64 binary. Both platforms use
`scripts/package-release.sh` to keep the installer's archive layout unchanged.

PR workflows use `pull_request_target` and reject fork heads before checkout.
Only branches in this repository can run source on the personal runners.
Description-only edits use a separate concurrency group and skip all jobs,
leaving running or pending code checks alone. The release workflow never
repeats CI on a PR head: it builds binaries when a release path changes and
tests the release PR through the CI workflow alone. E2E sandboxes wait for an API ping
before use and retain startup stderr for failure diagnostics. Update snapshots
assert the compiled version separately from the version-independent layout.
[CI and releases](docs/ci.md) records the setup and recovery commands.

## Rules that keep it honest

- **No polling for status.** Herdr only reports status transitions through
  per-pane `pane.agent_status_changed` subscriptions. The link subscribes to
  every agent pane on one connection and swaps it atomically: new
  subscription first, then the old one closes. Herdr sends each pane's
  current status when a subscription starts, so nothing falls between a
  snapshot and its subscription.
- **A refused subscription is not a lost server.** If a pane closes between a
  snapshot and the subscribe, Herdr rejects the whole request; the link keeps
  the connection and refreshes again.
- **Generations.** Every attachment to a pane has a generation. Frames and
  close messages from an older attachment are ignored, so switching threads
  quickly never paints the wrong pane.
- **Machines are independent.** A thread's id is `machine/pane`. Each machine
  has its own link and connection state; one going down removes only its
  threads, and the thread that was open on it re-opens when it comes back.
  Herdr's change counters are per server and never compared across machines.
- **No filesystem access in the app.** The link reads repository and branch
  for each thread's checkout (directly on this machine, with one batched git
  probe over SSH elsewhere) and hands them over with the snapshot.
- **Threads are tracked by identity.** The cursor follows its thread when the
  list reorders, and lands on a neighbour when the thread disappears.
- **Mouse hit testing uses drawn geometry.** Composer clicks use the same
  layout as rendering, including wrapped and scrolled text and picker rows.
  Keyboard and mouse choices share the same action handler. Composer clicks
  never become pane input, and modal dialogs and dictation swallow mouse events.
- **Wheel scrolling is independent of selection.** Updates and ticks keep the
  manual sidebar offset. Keyboard navigation resumes following the selected
  thread and keeps all three lines visible. The divider shows the scroll thumb.
- **Colour comes from Herdr, not UI literals.** `theme::load` reads Herdr's
  config at startup, honouring `HERDR_CONFIG_PATH` before the default path.
  Headings, ordinary actions, composer hints and thread metadata use `text`
  and `subtext0`; `accent` marks focus, selected entries and the orbit core. Opening the composer removes the previous discussion's accent.
  Status colours and small harness glyphs still use their palette tokens,
  and agent ANSI output is unchanged. Render tests assert these roles, and
  a real-binary PTY test changes the Herdr config between runs to check both
  orange and non-orange accents. Fills use a theme colour only when it is
  RGB (`Palette::raised`, `Palette::active_row`): an ANSI grey such as the
  plain `terminal` theme's `active_row_bg` is the colour terminals give dim
  text, so text drawn on it vanishes. There, the highlighted thread gets a
  thick accent edge instead of a fill.
- **The sidebar header has three bands.** The wordmark and stats (each
  group's count, before the filter, so the filter never hides how much is
  waiting), then the actions (New thread and the filter field), then the list,
  separated by hairline rules. `app::layout` names each header row.
- **Presets lead the composer.** The strip under the task shows every preset
  as a chip, in the order kept in `presets.json`, which is the user's order
  of preference: `PageUp`/`PageDown` move the focused chip one place earlier
  or later, keeping the cursor on it and emitting `SavePresets` immediately.
  `Ctrl+←`/`Ctrl+→` also reorder. Opening the composer for a new
  thread, and sending a task, set `start_preset`; every `settle` that knows
  what the machine has installed applies the first preset whose CLI is there,
  and the flag stays until a fresh inventory arrives from that machine or the
  user picks a harness, model, thinking level or preset by hand. So a preset
  is never applied before discovery can validate it, and a cached inventory
  from the last run never fixes the choice. Resending a thread keeps the
  thread's own choices instead.
- **Thinking belongs to the selected model.** Catalogs with per-model levels
  constrain both the picker and launch validation; changing models clears an
  incompatible level. Older catalogs keep their harness-wide levels. Codex
  uses `--config model_reasoning_effort=\"level\"`, replacing only that key in
  configured arguments and preserving unrelated overrides. Agent terminal
  colours are never recoloured to match Inbox's own frames.
- **OpenCode 2 takes its model through a session.** Its interactive command
  refuses `--model`, so for a catalog with `session_api` the plan strips
  `--model` from the arguments (a model pinned in `harness_args` becomes the
  session's model) and, once the workspace exists, runs `opencode api
  session.create` on the machine with the model and the workspace directory.
  The agent then starts with `--session <id>`. A `--server` in the arguments
  goes to the API call too, so the session lives where the TUI connects;
  `--standalone` with a chosen model is refused, since its private server
  never sees the session, and so are `--continue`, `--fork` and `--session`,
  which make OpenCode open another session; so is a machine whose OpenCode background service
  is disabled (`opencode service get disabled`, checked before any change),
  where every command gets a private server. The session is created with the
  executable from `harness_executables`, the one discovery read. The model's `provider/model` form is checked while
  planning, so a bad choice creates no worktree. A failed creation stops the
  launch before the agent starts; an agent that then fails to start has its
  session deleted (`session.remove`), since OpenCode keeps sessions until
  deleted. A startup dialog keeps it: the agent is running on it. The
  session's id is journaled as soon as it exists (`session_id`), so a launch
  interrupted before the agent starts still says which session it left. OpenCode 1 still gets `--model`.
- **Seen is local.** Herdr marks a finished pane seen when one of its own
  windows shows the tab. The inbox does not move Herdr's focus, so it keeps its
  own record: focusing a finished thread, or watching it finish, shows it as
  idle until its status changes again. Moving the list cursor shows the
  selected discussion without moving keyboard focus or marking it seen, so
  ready threads stay in place while browsing. Focusing the terminal marks the
  thread seen.
- **Compared agents never share files.** A task sent to several agents
  (off by default, `Compare` in the composer) is one independent launch per
  agent, always into a new worktree whose branch ends with that agent's
  harness and model. The composer plans every launch first, so one bad choice
  starts nothing; the journals carry the agent's place in the comparison, and
  only the first agent, the composer's own choice, updates the remembered
  defaults. Agent names mix the launch's second and its counter, so two
  launches in one send never collide.
- **Launches are never replayed blindly.** The journal records each stage. A
  prompt Herdr accepted without seeing a reaction is marked *unverified*,
  never sent twice. An agent stopped at a startup dialog keeps its task; once
  the user answers, the inbox waits until Herdr reports the agent idle and
  ready twice in a row (an agent can look idle while it is still starting)
  and only then sends it. Another dialog sends it back to waiting.
- **Tasks are never lost.** Every sent task goes to the history (`Ctrl+P` in
  the composer) before it launches. A launch that fails stays as a row until
  it is retried or dismissed; `e` brings its task back into the composer with
  its choices, any of which can change. A retry reuses the worktree the
  failed launch made, as a checkout, since its branch already has one, and
  sending it retires the failed launch: its workspace closes, leaving the
  worktree to the retry, and its journal stays on disk until the retry ends,
  so a crash mid-retry still shows the failed launch on restart.
- **Dictated words are never lost.** Recording refuses to start without a
  way to transcribe; a delivery that fails keeps the transcript in its
  notice; while the microphone is open no key reaches an agent.
- **Secrets stay out of sight.** API keys go to `curl` on its standard input,
  never in its arguments, and a key being typed is drawn as dots.
- **The API is a second composer, not a second launcher.** A prompt from the
  API goes through `App::send` like a typed task, so it can never launch
  differently. It answers once the launch is planned, not when the agent is
  up. A repository not found starts a project scan, since scans otherwise
  follow the composer opening. The event loop claims a prompt before handing
  it to the app, and a request that timed out first withdraws it, so a
  prompt reported as failed never launches later. The composer knows projects
  by name, so a repository whose checkout shares its name with another project
  is refused rather than launched in the wrong one. Without `[api] enabled`
  and both an origin and a login, nothing listens.
- **Discovery, launches and self-updates never block the UI.** They run on
  worker threads and report back as inputs.
- **Usage comes from the CLIs' own channels.** Codex is asked through its
  documented app server; Claude Code reports through the status line it runs,
  so the inbox gives every Claude thread it launches a wrapper that caches
  those numbers and still runs the user's own status line. The inbox never
  reads or sends a provider token. The Auto harness is resolved on every
  settle from the last report, so a send always has a concrete harness even
  before any numbers exist.
- **Updates are explicit and local.** Ctrl+G in the list or composer checks the
  latest stable GitHub release; Enter confirms the exact tag shown. The archive
  and SHA-256 must both be published for this platform. The installer verifies
  the checksum and the binary's version before an atomic rename next to the
  current executable. It never updates Herdr, settings or remote machines.
  Closing the dialog keeps in-flight work; reopening cannot start duplicate
  installs. A successful update waits for a user restart.
- **Spaces type immediately by default.** Ctrl+T dictates; only an explicit
  `[speech] space_hold = true` enables key-repeat detection and its wait.
- **Forwarding a key does not spend a frame.** A terminal key whose only
  effects send input to the agent does not invalidate the screen, unless it
  also changes focus or opens a dictation menu. Its returned ANSI frame does,
  preserving the 16 ms limit for streaming output and any redraw already
  pending. Clock-driven tests replay keys and echoes through this same scheduler;
  PTY tests measure paused spaces and typing during continuous agent output.
- **The emulator cannot crash the app.** vt100 panics on some edge cases (a
  wrap in a one-row screen); the screen keeps at least 2×2 cells, catches a
  panic while parsing, and resets until the next full frame.
