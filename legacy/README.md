# Herdr Inbox (legacy plugin)

> This is the original Python plugin, kept while the standalone `herdr-inbox`
> client at the repository root replaces it. New work happens there.

A [Herdr](https://herdr.dev) plugin that turns your terminal into a control
room for coding agents: start a thread on any project, harness, model, machine,
or Git worktree from one composer, then follow every agent on every machine in
one live inbox.

- **One composer, every harness.** Claude, Codex, OpenCode, Pi, Gemini, Copilot,
  Amp, Cursor, Grok, Kimi, Kiro, Droid, Hermes, Kilo, Qwen, Cline, and the rest
  of Herdr's supported kinds, each with its CLI's own model and thinking catalog.
- **Worktree-first.** Every thread can run in its own Git worktree with a
  branch named after the task, so a dozen agents work on one repository without
  touching each other's files.
- **Never waits.** Sending runs in the background while you write the next
  task. Ctrl+Enter sends the same task again to another harness or machine.
- **Live, not polled.** The inbox subscribes to Herdr's socket events, locally
  and over one persistent SSH session per saved machine. A status change shows
  up in milliseconds.
- **Dictate.** Ctrl+T records, transcribes offline or through a service you
  connect in two keystrokes, and sends.
- **Stdlib only.** Python 3.9+, no packages, no build step, no daemon.

## Install

Herdr 0.9.1 or newer, and Python 3.9 or newer on each machine that runs agents.

```sh
herdr plugin install lucasscariot/herdr-inbox
```

Then bind the two actions in `~/.config/herdr/config.toml`. `prefix+c` is
Herdr's default for a new tab, so remove it from `keys.new_tab` first, or pick
other keys.

```toml
[[keys.command]]
key = "prefix+c"
type = "plugin_action"
command = "lucasscariot.herdr-inbox.new"
description = "New agent thread"

[[keys.command]]
key = "prefix+j"
type = "plugin_action"
command = "lucasscariot.herdr-inbox.open"
description = "Agent inbox"
```

Run `herdr server reload-config`, then reload the client configuration or
reattach. Both actions are also in Herdr's plugin action palette. To update,
run the install command again, then close any open composer with Escape and
reopen it.

For development, link a checkout instead:

```sh
git clone https://github.com/lucasscariot/herdr-inbox
herdr plugin link "$PWD/herdr-inbox"
python3 -m unittest discover -s herdr-inbox/tests
```

## Quick start

1. Press **Ctrl+Space, then C**. The composer opens with the project you used
   last, on the machine and harness you used for it.
2. Type the task. Pick a different project, harness, or model with F2, F3, F4,
   or click the controls. Every picker is fuzzy: `bgpk` finds
   `opencode/big-pickle`.
3. Press **Enter**. The composer clears at once; a line underneath shows the
   launch creating its workspace, starting the harness, and sending the task.
4. Press **Ctrl+Space, then J** to see every agent, grouped by what it needs
   from you. Press Enter on a row to jump into its terminal.

## The composer

```
What should we build in [ Project: cockpit ▾ ]
[ Preset: Claude · Fable ▾ ]                                   [ + Save preset ]
╭─ Task ─────────────────────────────────────────────────────────────────────╮
│  Fix the login redirect loop on mobile                                      │
│                                                                             │
│ [ Harness: Claude ▾ ] [ Model: Fable ▾ ] [ Thinking: Max ▾ ]    [ Send ↑ ]  │
╰─────────────────────────────────────────────────────────────────────────────╯
[ Machine: Local ▾ ]  [ Workspace: New worktree ▾ ]  git worktree of ~/Work/cockpit as fix-login-redirect-loop-mobile
⟳ cockpit · claude · Fix the login redirect loop — starting claude
```

- **Project** lists every Git repository found on your machines. Linked
  worktrees are grouped under their repository, wherever they live on disk.
- **Machine** lists the hosts where that project exists: Local plus Herdr's
  enabled saved SSH machines.
- **Harness** lists the agent CLIs installed on the chosen machine.
- **Model** and **Thinking** come from the installed CLI's own catalog. Type a
  native model ID and choose **Use** when it is missing from the list.
- **Workspace** decides where the agent runs: the main checkout, an existing
  worktree, or a **new worktree** whose branch is named after the task or by
  you. Worktrees are created by Herdr on the selected machine, under
  `~/.herdr/worktrees/<repo>/<branch>`.
- **Preset** applies a saved harness, model, and thinking level together.
  Choose a model, press **Ctrl+D**, name it. **Ctrl+R** renames and **Delete**
  removes inside the list.

Machine, harness, and workspace mode are remembered per project. Models and
thinking levels are remembered per project, machine, and harness, so switching
hosts never carries over another host's model.

| Key | Action |
| --- | --- |
| Enter | Send the task and clear the composer |
| Ctrl+Enter or Ctrl+S | Send and keep the task for another launch |
| Alt+Enter or Shift+Enter | Insert a newline |
| Ctrl+T | Dictate: speak, then Enter sends or Ctrl+T stops to edit |
| F10 | Dictation menu: connect a service, build whisper.cpp, disconnect |
| Ctrl+P / Ctrl+N | Recall previous and next tasks from history |
| Tab / Shift+Tab | Move between controls |
| F2 F3 F4 F6 F7 F8 F9 | Project, harness, model, machine, preset, thinking, workspace |
| Ctrl+D | Save the current harness, model, and thinking as a preset |
| F5 | Rescan projects, refresh model catalogs, resnapshot every machine |
| Esc | Close a picker, or close the composer |

Mouse clicks work on every control. The task box uses the terminal's real
cursor, wraps at word boundaries, and accepts multiline paste. A failed launch
stays listed with its error. Closing the composer leaves agents running.

## The inbox

```
NEEDS INPUT
● Fix login redirect             cockpit › fix-login-redirect   Local · Claude     2m
READY
✓ Review navigation              vibe-tshirt › main             MacBook · Codex    8m
WORKING
◐ Add invoice export             veezu-server › main            Mac Studio · Pi    14s
```

Threads waiting for input come first, then finished ones, then working and
idle ones. Each row shows the task, the project and branch, the machine and
harness, and how long ago the agent's state last changed.

| Key | Action |
| --- | --- |
| ↑ ↓ or j k | Move |
| Enter or click | Open the thread's terminal |
| r | Reply to an idle or finished agent without leaving the inbox; Ctrl+T dictates it |
| e | Fill the composer from a thread's launch to send it again elsewhere |
| d | Dismiss a failed launch |
| / | Filter by task, project, branch, harness, or machine |
| n | New thread |
| Esc | Clear the filter, or close |

When a harness stops at a startup dialog, such as a folder-trust prompt in a
new worktree, the launch keeps the task and the thread appears under *Needs
input*. Answer the dialog in the terminal; the task is sent as soon as the
agent is idle. A task that was sent but never acknowledged shows `?` until the
agent changes state.

The header shows one dot per machine: live, connecting, or unavailable with the
reason in the footer. Links reconnect with backoff on their own.

## Dictation

Press **Ctrl+T**, speak, then **Enter** to send the transcribed task, or
**Ctrl+T** again to edit it first. Escape discards the recording. Recording
uses PipeWire, ALSA, PulseAudio, sox, or ffmpeg, whichever is installed.

While the microphone is open, a live equalizer shows what it hears: the
composer, the inbox, the popup, and the thread-only client's mode bar all draw
the same bars, so you can tell at a glance that your voice is coming through.
If nothing audible arrives for a few seconds, the meter says so, which is
usually a muted microphone or the wrong input device.

### Dictate into a running thread

The same flow works for a follow-up on any thread, not only the composer. Bind
the plugin's `dictate` action to a key; `ctrl+t` matches the composer:

```toml
[[keys.command]]
key = "ctrl+t"
type = "plugin_action"
command = "lucasscariot.herdr-inbox.dictate"
description = "Dictate to this agent"
```

Press it while an agent's terminal is focused. A small popup records; **Enter**
transcribes and submits the text to that agent through Herdr's prompt
transport, **Ctrl+T** types it into the agent without sending so you can edit
it, and **Esc** discards it. A thread that is waiting for your approval is not
interrupted: the popup reports it and shows the transcript instead. The
thread-only client below needs no binding; its Ctrl+T works everywhere.

`python3 main.py dictate --pane <id> [--machine <id>]` is the same engine for
scripts: it prints JSON events and takes one line on stdin (`send`, `type`,
or `cancel`). While it records it prints `level` events with the meter's band
levels (`bands`, 0 to 1) and a `quiet` flag, fifteen times a second.

The first Ctrl+T, or **F10** at any time, opens the Dictation menu. No key is
ever typed into a file:

- **A local transcriber already installed** is detected and preferred:
  `voxtype`, or whisper.cpp's `whisper-cli` with a `ggml-*.bin` model in the
  usual places. Offline, nothing to configure.
- **Groq Whisper**, **Google Gemini**, **OpenAI**, **Mistral Voxtral**, or
  **Deepgram**: the provider's key page opens in your browser, you paste the
  key into a hidden field, the plugin verifies it with one request and stores
  it with owner-only permissions in the plugin state directory. Groq and
  Gemini have free tiers.
- **Build whisper.cpp here**: clones and builds whisper.cpp into
  `~/.local/share/herdr-inbox` and downloads the `small` model (about 470 MB).
  Needs git, cmake, and a C++ compiler. Linux and macOS.
- **Custom command**: any transcriber that prints text for `{file}`.

Keys in `config.json` under `"speech": {"keys": {...}}` and the usual
environment variables (`GROQ_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY`,
`MISTRAL_API_KEY`, `DEEPGRAM_API_KEY`) also work. `"speech": {"language":
"en", "model": "…"}` is passed through; `model` may be a whisper.cpp model
path for a local tool.

## Configuration

`herdr plugin config-dir lucasscariot.herdr-inbox` prints the directory. Everything in
`config.json` is optional.

```json
{
  "roots": ["~/Work", "~/Projects"],
  "depth": 2,
  "default_workspace": "worktree",
  "branch_prefix": "",
  "speech": {"language": "en"},
  "harness_args": {"codex": ["--full-auto"]},
  "harness_executables": {"cursor": "cursor-agent"},
  "models": {"codex": ["your-model-id"]},
  "machines": {
    "Mac Studio": {
      "roots": ["~/code"],
      "harness_args": {"codex": ["--no-daemon"]}
    }
  },
  "projects": [
    {"name": "api", "machine": "Mac Studio", "path": "~/other/api"}
  ]
}
```

| Key | Meaning |
| --- | --- |
| `roots`, `depth` | Where discovery looks and how deep. It recognizes Git repositories and common manifests, skips dependency folders, and stops at the first project. Defaults: `~/Work`, `~/Projects`, `~/goinfre`, depth 2. |
| `default_workspace` | `checkout` or `worktree` for projects you have not launched before. |
| `branch_prefix` | Prepended to branch names derived from tasks, for example `yourname/`. |
| `harness_args` | Native CLI arguments per harness. |
| `harness_executables` | Executable name to look for per Herdr agent kind. |
| `models` | Extra model IDs per harness. |
| `machines` | Per-machine overrides of the three keys above plus `roots` and `depth`, keyed by saved label or profile ID. |
| `projects` | Repositories outside the roots. A name distinguishes unrelated repositories that share a directory name. |
| `speech` | Dictation settings; the F10 menu is the usual way to connect a service. |

State lives in Herdr's plugin state directory: presets, per-project
preferences, task history, cached inventories, dictation credentials, and one
journal per launch under `threads/`.

## How it works

Each launch is journaled before its first mutation and after every step, so an
interrupted launch is never replayed and shows up in the inbox with its error:

1. Create the workspace with `workspace create` in the chosen checkout, or
   `worktree create` for a new branch, on the selected machine.
2. Rename the tab after the task and start the harness in the workspace's pane
   with its native model, thinking, and configured arguments.
3. Submit the task. OpenCode 2 receives it through its session API with exact
   session and message IDs; other harnesses receive it through Herdr's prompt
   transport.

Agent state comes from Herdr's socket events through `inbox/relay.py`, a
self-contained watcher that runs in-process for the local server and is shipped
over SSH to saved machines, where it needs only Python 3.9. Pane lifecycle
events refresh the agent list, per-pane status subscriptions deliver
transitions, a periodic reconcile guards against anything missed, and
`inbox/live.py` restarts a link that drops. Remote mutations use Herdr's
saved-machine forwarding; discovery, OpenCode calls, and event links use SSH
with existing keys, strict host checks, and a shared control socket. Nothing
is ever redirected to Local when a host is unavailable. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the module map.

## Scripting

```sh
python3 main.py ui --demo                 # preview the composer without Herdr
python3 main.py ui --demo --view inbox    # preview the inbox
python3 main.py discover                  # projects, worktrees, harnesses, models per machine
python3 main.py list                      # the live inbox as JSON
python3 main.py resume                    # send tasks that waited on a startup dialog
python3 main.py launch --machine Local --project cockpit --harness claude --model fable --worktree --task 'Fix the login redirect loop'
python3 main.py launch --machine 'Mac Studio' --project api --harness codex --checkout ~/other/api --task 'Review the current diff'
```

`--worktree` takes an optional branch name and `--base` an optional base ref.
Except for demo mode, run these inside Herdr so they have a session socket.

## Optional thread-only client

Plugins cannot remove Herdr's built-in Machines block. `patches/` carries an
opt-in patch for Herdr 0.9.3 that adds a `HERDR_INBOX_MODE=1` client mode with
a full-height thread sidebar and a New thread button, hiding machine and
workspace navigation and the tab bar.

Each row shows the project with a status badge, the task, and the branch with
the harness (and the machine, for threads off Local). Sorted by priority, the
list is grouped under "needs input", "ready", "working", and "idle" headings.
Right-click a row for "Archive thread", or click "archive" on the thread that
is open: this closes the thread's workspace on its machine and keeps a linked
worktree checkout on disk. Archiving asks first unless Herdr's
`ui.confirm_close` is off.

Build it with Rust, `just`, and Zig:

```sh
python3 scripts/build-client.py --zig /path/to/zig
HERDR_INBOX_MODE=1 build/herdr-inbox-client
```

The pinned upstream commit is recorded in the build script; the patch ships
with the upstream Apache 2.0 license. Regular `herdr` keeps working.

The client is built for the keyboard loop *pick a thread, say what to do next,
move on*:

| Key | Action |
| --- | --- |
| Tab | Move focus between the thread list and the agent's terminal (Shift+Tab always reaches the agent) |
| j k or ↓ ↑, g G | Move through the threads; the mode bar shows `THREADS` while the list has focus |
| Enter or o | Open the thread under the cursor and return focus to its terminal |
| Backspace, Delete, or x | Archive the thread under the cursor (closes its workspace; the worktree stays on disk) |
| Ctrl+T | Dictate into the focused thread, or the thread under the cursor: Enter sends, Ctrl+T types without sending, Esc discards |
| Esc | Leave the thread list |

Tab is intercepted only while an agent's terminal is focused; the composer and
other plugin panes keep their own Tab. Ctrl+T in the composer still dictates
the task. The client finds the plugin through `herdr plugin list`; set
`HERDR_INBOX_ROOT` to point it at a checkout instead.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md)
for how to run the test suite, preview the UI offscreen, and validate against
an isolated Herdr session without touching your real one.

## License

[MIT](LICENSE). The Herdr client patch under `patches/` modifies Apache 2.0
code from [herdrdev/herdr](https://github.com/herdrdev/herdr).
