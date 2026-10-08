# Architecture

Herdr Inbox is a Herdr plugin: Herdr runs `python3 main.py …` in a plugin pane
with `HERDR_SOCKET_PATH`, `HERDR_PLUGIN_STATE_DIR`, and `HERDR_PLUGIN_CONFIG_DIR`
set. Everything is standard-library Python 3.9+.

```
main.py              entrypoint: ui | open | discover | list | resume | launch
inbox/ui.py          curses UI: composer and inbox views, key decoding, drawing
inbox/text.py        cell widths, word wrapping, fuzzy ranking, formatting
inbox/banner.py      the Braille knot and wordmark (cached per animation frame)
inbox/herdr.py       Herdr transport: CLI calls, local socket requests, SSH command builder
inbox/inventory.py   project and worktree discovery probe, harness and model catalogs
inbox/threads.py     the launch state machine, startup-dialog resume, inbox rows
inbox/opencode.py    OpenCode 2 session API calls
inbox/relay.py       self-contained event watcher (runs locally and on remote hosts)
inbox/live.py        one supervised relay link per machine, feeding a queue
inbox/speech.py      dictation: recorders, transcription services, local tools
inbox/dictate.py     dictation into a running thread: headless engine and popup
inbox/presets.py     named harness/model/thinking combinations
inbox/store.py       config, preferences, presets, credentials, journals
```

## Data flow

**Discovery.** `inventory.PROBE` is a Python script sent to each machine
(locally with `python3 -c`, remotely over SSH). It walks the configured roots,
groups linked worktrees under their repository by reading `.git` pointers, and
asks each installed harness CLI for its model catalog. Results are cached per
machine in the state directory; catalogs are reused for 15 minutes.

**Launch.** `threads.launch` resolves the workspace choice, journals its intent,
then creates the workspace or worktree, renames the tab, starts the harness,
and submits the task, journaling after each step. A startup dialog turns the
record into `startup_blocked`; `threads.resume` sends the task once the agent
is idle. Failures become `needs_attention` rows the inbox can dismiss or
relaunch.

**Events.** `relay.Relay` opens one lifecycle subscription and one status
subscription (every known agent pane) on dedicated socket connections, because
Herdr resets a subscription connection that receives any other request. It
emits `snapshot`, `agent`, `gone`, `ping`, and `error` lines. `live.Link` runs
it in-process for Local and as `python3 -c <source>` over SSH for saved
machines, restarts it with backoff, and pushes messages into a queue the UI
drains on every loop iteration. The UI rebuilds rows only for machines that
changed.

**Dictation.** `speech.Recording` captures 16 kHz mono WAV with the first
available recorder. `speech.transcribe` picks the first usable backend: a
configured command, a detected local tool, then hosted services by key.
Credentials saved from the Dictation menu live in `credentials.json` with
mode 0600 and override `config.json`.

## Invariants worth keeping

- Every mutation on a remote machine goes through Herdr's `--machine`
  forwarding or an explicit SSH command; nothing falls back to Local.
- A launch is journaled before its first mutation; an uncertain submission is
  never replayed.
- `relay.py` imports nothing from the package so it can be shipped as source.
- The UI thread never blocks on I/O: launches, transcription, verification, and
  installs run in the executor; events arrive through the queue.
