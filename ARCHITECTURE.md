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
| `discovery` | An embedded Python probe, run with `python3` on each machine (over SSH through a login shell), finds projects, worktrees, installed agent CLIs and their model and thinking catalogs. Models are cached 15 minutes. |
| `launch` | Plans a launch (branch name, flags) without side effects, then runs it step by step with the `herdr` CLI: worktree or workspace, tab title, `agent start`, thread metadata, `agent prompt`. A journal is written before the first change and after each step. |
| `editor`, `fuzzy` | The composer's text box and its pickers' ranking. |
| `presets` | Named harness, model and thinking combinations, in the plugin's `presets.json` format. |
| `speech` | Dictation: `recorder` (whatever is installed, stopped with SIGINT so the WAV is finalized), `meter` (an FFT over the file being written, twelve bands, an adaptive noise floor), `backends` (a command, voxtype, whisper.cpp, or a hosted service through `curl` with the key on stdin), `whisper` (builds whisper.cpp and downloads a model). |
| `app` | The state machine. `update(Input) -> Vec<Effect>`; no I/O, so every behaviour is unit-tested. |
| `threads` | Turns a snapshot into labelled, grouped, sorted threads; tracks when statuses changed and which finished threads the user has seen. |
| `screen` | A vt100 emulator fed with Herdr's frames, drawn into ratatui cells. |
| `keys` | Encodes key presses as xterm bytes for the pane, honouring its cursor-key and bracketed-paste modes. |
| `git` | Reads repository name and branch from `.git` files, without running git. |
| `hold` | Telling a held space bar from typed spaces by key-repeat timing. Pure. |
| `orbit` | The orbit logo: the fleet as rings and beads, rendered to Braille cells. Pure, a function of the time. |
| `theme` | Herdr's built-in palettes and `config.toml` overrides. |
| `ui` | Pure drawing from `&App`. |
| `runtime` | Terminal setup, the event loop, effect execution. |

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
- **Seen is local.** Herdr marks a finished pane seen when one of its own
  windows shows the tab. The inbox does not move Herdr's focus, so it keeps its
  own record: opening a finished thread, or watching it finish, shows it as
  idle until its status changes again.
- **Launches are never replayed blindly.** The journal records each stage. A
  prompt Herdr accepted without seeing a reaction is marked *unverified*,
  never sent twice. An agent stopped at a startup dialog keeps its task; once
  the user answers, the inbox waits until Herdr reports the agent idle and
  ready twice in a row (an agent can look idle while it is still starting)
  and only then sends it. Another dialog sends it back to waiting.
- **Dictated words are never lost.** Recording refuses to start without a
  way to transcribe; a delivery that fails keeps the transcript in its
  notice; while the microphone is open no key reaches an agent.
- **Secrets stay out of sight.** API keys go to `curl` on its standard input,
  never in its arguments, and a key being typed is drawn as dots.
- **Discovery and launches never block the UI.** They run on worker threads
  and report back as inputs.
- **The emulator cannot crash the app.** vt100 panics on some edge cases (a
  wrap in a one-row screen); the screen keeps at least 2×2 cells, catches a
  panic while parsing, and resets until the next full frame.
