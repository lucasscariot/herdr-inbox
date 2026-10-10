# Herdr Inbox

A dedicated terminal client for [Herdr](https://herdr.dev), built for people
who run many coding agents across many projects. Every agent thread sits in
one list, sorted by what it needs from you. The thread you pick runs live next
to it, fully interactive.

```
 herdr inbox                    1/3 │ ✻ Claude Code
 1 needs input · 1 ready            │
  +  New thread                  n  │ > Fix the login redirect loop on mobile
 / Filter threads                   │
 NEEDS INPUT  1                     │   Do you want to make this edit?
▎cockpit                    ● input │   ❯ 1. Yes
▎Fix the login redirect loop on mo… │     2. No
▎✻ Claude · ⎇ fix-login-redirect    │
                                    │
 READY  1                           │
▎site                       ✓ ready │
▎Review navigation                  │
▎◈ OpenCode · ⎇ main             2m │
                                    │
 WORKING  1                         │
▎api                      ◐ working │
▎Add invoice export                 │
▎◆ Codex · ⎇ main               14s │
  AGENT  tab threads
```

Herdr Inbox is its own program and leaves your Herdr setup alone. It talks to
the Herdr server you already run, through Herdr's public interfaces only:

- the JSON socket API for threads, status and archiving;
- `herdr terminal session control` for the live terminal, Herdr's documented
  interface for third-party clients.

It never replaces `herdr`, never starts a server unless you ask it to, and
only updates its own binary when you confirm an update. Plain `herdr` keeps
working beside it, on the same sessions.

## Install

You need [Herdr](https://herdr.dev) 0.9.2 or newer.

```sh
curl -fsSL https://raw.githubusercontent.com/lucasscariot/herdr-inbox/main/install.sh | sh
```

The installer downloads the prebuilt binary for Linux or macOS (x86_64 or
arm64), checks its SHA-256, and puts it in `~/.local/bin`
(`HERDR_INBOX_INSTALL_DIR` to change it). It never touches `herdr`. To update,
press **Ctrl+G** in the thread list or composer, or run the installer again.

To build from source instead (Rust 1.88 or newer):

```sh
cargo install --locked --git https://github.com/lucasscariot/herdr-inbox
```

To uninstall, remove the binary and, if you like, what it remembers:

```sh
rm ~/.local/bin/herdr-inbox
rm -rf ~/.config/herdr-inbox ~/.local/state/herdr-inbox ~/.local/share/herdr-inbox
```

## Use

```sh
herdr-inbox                  # the default Herdr session
herdr-inbox --session work   # a named session
```

If no Herdr server is running, Herdr Inbox offers to start one. That runs
`herdr server`, exactly what `herdr` itself would do.

| Where | Key | Action |
| --- | --- | --- |
| Threads | `j` `k` / `↓` `↑` | Move and show the selected discussion |
| | `g` `G` / `Home` `End` | Show the first / last discussion |
| | `PageUp`, `PageDown` | Move by a page of threads |
| | Mouse wheel | Scroll without moving the selection |
| | `Enter`, `o`, click | Focus the selected agent |
| | `n`, click **+ New thread** | Open the composer |
| | `r` | Reply to the agent without opening it |
| | `e` | Send the thread's task again, from the composer |
| | `/`, click **Filter threads** | Filter by title, project, branch, harness or machine; `Esc` clears |
| | `x`, `Delete`, `Backspace` twice | Archive: closes the thread's workspace, keeps its worktree on disk. The second press (or `Enter`, `y`) confirms; `Esc` cancels |
| | `d` | Dismiss a failed launch |
| | `Ctrl+T` | Dictate to the thread under the cursor |
| | `F10` | Dictation settings |
| | `Ctrl+G` | Check GitHub releases and update Inbox |
| | `Tab`, `Esc` | Back to the agent |
| | `q`, `Ctrl+C` | Quit |
| Agent | anything | Goes to the agent, including `Ctrl+C` and `Shift+Tab` |
| | `Ctrl+T` | Dictate to the agent |
| | `Tab` | Back to the threads |

Threads are grouped as **needs input**, **ready** (finished, not yet focused),
**working** and **idle**. The most recent change in each group comes first.

Moving through tasks shows each discussion immediately, without Enter.
Keyboard focus stays in the list until you press Enter or Tab, or click the
agent. Previewing a finished task does not mark it seen or move its row;
focusing the agent does.

A small coloured glyph marks each thread's harness before its name: ✻ for
Claude Code, ◆ for Codex, π for Pi and ◈ for OpenCode. Other harnesses get a
neutral ❯ and their own name. The harness name comes before the branch, so a
long branch cannot hide which agent is running. A position counter and a
scroll thumb show where you are in the list.

Below the composer's controls when there is room, and when no thread is open,
the orbit turns: the core is the inbox, each ring a machine, each bead a
thread in its status colour. The core breathes while a thread needs input,
and a ring goes dashed while its machine connects or red when it is
unreachable.

Herdr Inbox uses your Herdr theme: the same built-in themes, `[theme] name`,
and `[theme.custom]` colours from `~/.config/herdr/config.toml`. Inbox's own
frames and popup titles use that palette; the live agent terminal keeps the
agent's original colours.

### Updating Inbox

Press **Ctrl+G** in the thread list or composer. From an agent, press Tab
first; Ctrl+G inside an agent still goes to the agent. Updates also work
without a running Herdr server.

The dialog checks GitHub's latest stable release and shows your running
version, the release version and its notes. Arrow keys scroll the notes,
**B** opens that exact release in your browser, and **R** checks again.
Checking never installs anything. Press **Enter** to confirm an available
update, or Escape to return to your draft or thread.

Inbox downloads the binary for this machine, verifies its SHA-256 and version,
then replaces the binary you are running, including a custom install directory.
A failed download or verification leaves the old binary intact. Restart Inbox
after a successful update; your agents keep running. Nothing updates Herdr,
your settings or saved SSH machines. The dialog needs `curl` to check GitHub.
It does not install prereleases, unreleased `main` builds or older versions.

### Starting a thread

Press `n` or click **+ New thread**. Write the task, check the choices under
it, press `Enter` or click **send**. Click in the task to place the cursor,
click a field to open its choices, then click a choice. The mouse wheel moves
through a choice list. The task stays near the top even in a tall window.

The composer clears at once and stays open for the next task; the launch runs
in the background and its progress shows under **Launches**.

| Key | Action |
| --- | --- |
| `Enter` | Send |
| `Ctrl+S`, `Ctrl+Enter` | Send and keep the task, to send it again elsewhere |
| `Shift+Enter`, `Alt+Enter` | New line |
| `Tab`, `Shift+Tab` | Move between the task and the choices |
| `F2` `F6` `F3` `F4` `F8` `F9` | Project, machine, harness, model, thinking, workspace |
| `F12` | Compare: send the task to two or three agents at once |
| `Ctrl+T` | Dictate the task |
| `Ctrl+V`, the desktop's paste | Paste a screenshot as `[Image #1]` |
| `Ctrl+P`, `Ctrl+N` | Previous and next tasks from the history |
| `Ctrl+D` | Save the harness, model and thinking level as a preset |
| `F5` | Rescan projects and model catalogs |
| `Ctrl+G` | Check GitHub releases and update Inbox |
| `Esc` | Close a list, then the composer (the draft stays) |

**Presets** (`F7`) apply a saved harness, model and thinking level in one
move. In the preset list, `Ctrl+R` renames and `Delete` removes.

Every list filters as you type: `bgpk` finds `opencode/big-pickle`. A model
the list does not know can still be used: type its id and pick **Use …**.

- **Project** lists the git repositories (and folders with a project manifest)
  found under your roots on every machine; linked worktrees are grouped under
  their repository.
- **Machine** lists the machines that have the project.
- **Harness**, **Model** and **Thinking** come from the agent CLIs installed
  on that machine and their own model lists. Models match both their display
  name and their id. Codex's GPT models offer the reasoning levels reported
  in its `models_cache.json`, including new levels; `CODEX_HOME` is respected.
  Choosing a different model clears an incompatible thinking level. **Default**
  leaves the CLI's settings alone. If no levels are reported, the Thinking row
  says so rather than silently disappearing. `F5` refreshes the catalog.
- **Workspace** is a new git worktree by default, its branch named after the
  task (shown as `⎇ branch`), or one you name, or an existing checkout.
  Herdr creates worktrees under `~/.herdr/worktrees/<repo>/<branch>`.

**Comparing agents.** Off by default, the **Compare** row (`F12`) sends the
same task to two or three agents at once, to compare their results. Add a
preset or an installed harness with its default model; pick it again to drop
it; **Off** goes back to one agent. The composer's own harness, model and
thinking level is the first agent. Every compared agent gets its own new
worktree so they never touch each other's files: the branches share the
task's name and end with the agent's harness and model, shown as
`⎇ fix-login-{claude,codex-gpt-5}`, and a checkout cannot be chosen while
comparing. Each agent appears under **Launches** and in the thread list with
its branch. A comparison is an experiment on one task: the next task goes to
one agent again, and only the first agent's choices are remembered for the
project.

**Screenshots.** Paste an image into the task or a reply (`r`) and it shows
as `[Image #1]`. It is saved under `~/.cache/herdr-inbox/images` (kept 30
days) and reaches the agent the way its own paste would: Claude Code and Codex
attach it, other agents get its path. In an open thread the agent reads the
clipboard itself. Images work for threads on this machine; on a saved SSH
machine the agent would get a path it cannot open. On Linux this needs
`wl-clipboard` (Wayland) or `xclip` (X11).

The composer remembers per project which machine, harness and workspace mode
you used, and per machine and harness which model and thinking level, so
switching hosts never carries over another host's model.

If an agent stops at a startup dialog (a folder-trust or login prompt), the
thread shows under **needs input**. Answer the dialog in the thread and the
task is sent as soon as the agent is ready.

#### Configuration

`~/.config/herdr-inbox/config.toml`, every key optional (until it exists, the
legacy plugin's `config.json` is read):

```toml
roots = ["~/Work", "~/Projects"]   # where projects are found
depth = 2                          # how deep to look, 0 to 3
branch_prefix = "lucas/"           # prepended to new branch names
default_workspace = "worktree"     # or "checkout"
agent_start_timeout_ms = 45000

[harness_args]                     # extra flags per agent CLI
codex = ["--no-daemon"]

[harness_executables]              # when a CLI has another name
claude = "claude-dev"

[models]                           # model ids to add to a CLI's list
claude = ["claude-opus-5-5"]

[[projects]]                       # repositories outside the roots
path = "/srv/api"
machine = "Mac Studio"

[machines."Mac Studio"]            # per machine: replaces, never merges
harness_args = { codex = ["--no-daemon", "--fast"] }
```

### Dictation

Press `Ctrl+T` to talk, anywhere: the composer's task, an agent's prompt,
a reply, or the thread under the cursor. Press `Ctrl+T` again to type the
words unsent, so you can read them before pressing Enter. Ordinary spaces
type immediately and never start dictation by default.

Hold-to-dictate is optional. Enable it in `~/.config/herdr-inbox/config.toml`:

```toml
[speech]
space_hold = true
```

With this setting, hold Space to talk and let go to type the words unsent.
Herdr Inbox detects a hold from keyboard auto-repeat, so a tapped space
followed by a pause waits about 700 ms initially. The next key types any
waiting spaces before itself. After a hold, the wait adapts to your keyboard's
repeat delay. Omit `space_hold` or set it to `false` for immediate spaces;
Ctrl+T still works.

While the status bar shows `● REC` and the live meter:

| Key | Action |
| --- | --- |
| Let go of space | Stop and type the words, unsent (when holding space) |
| `Enter` | Stop and send: launch the task, send the reply, or prompt the agent |
| `Ctrl+T` | Stop and type the words, unsent |
| `Esc` | Discard |

Nothing you type reaches an agent while the microphone is open. If the words
cannot be delivered (the agent is waiting for an answer, say), the notice
keeps them so you can paste them.

`F10` sets up transcription. Pick a service and paste its key (Groq and
Gemini have free tiers), or build whisper.cpp locally for fully offline
dictation, or give any command that prints a transcript for `{file}`.
Installed `voxtype` or `whisper-cli` with a model are found on their own.
Recording uses `pw-record`, `arecord`, `parecord`, `sox` or `ffmpeg`,
whichever is installed; hosted services are called with `curl`.

`[speech]` in the config can force a backend, pin a language or model, turn
on `space_hold`, or hold keys; `GROQ_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY`,
`MISTRAL_API_KEY` and `DEEPGRAM_API_KEY` work too.

### Coming from the plugin

Herdr Inbox used to be a Herdr plugin plus a patched Herdr client. It is now
this one program. On first start it copies the plugin's remembered choices,
presets, task history and dictation keys, and reads its `config.json` until a
`config.toml` exists; nothing in the plugin's directories is changed. To move
over:

1. Install with the line above. It replaces the old `herdr-inbox` command.
2. Remove the plugin: `herdr plugin uninstall lucasscariot.herdr-inbox` (or
   `herdr plugin unlink …` for a linked checkout), delete the
   `lucasscariot.herdr-inbox.*` key bindings from `~/.config/herdr/config.toml`,
   then `herdr server reload-config`.
3. Delete the patched client: `rm -r ~/.local/share/herdr-inbox/bin`. A
   whisper.cpp built by the plugin next to it is reused as is.

### Other machines

Every enabled machine you saved in Herdr (`herdr machine add`, listed by
`herdr machine list`) joins the same list. Each remote row names its machine,
and the status bar shows each machine's state: `●` live, `◌` connecting, `✗`
unreachable. Opening a remote thread streams it live over SSH, and typing
reaches it the same way.

Herdr Inbox reaches a machine with your own SSH setup, one shared connection
per host. On the remote side it needs `herdr` on the PATH of a login shell, or
in `~/.local/bin`, `~/.cargo/bin`, Homebrew or mise. A machine that cannot be
reached never blocks the others. Without a local Herdr server, the remote
threads stay usable, and `s` starts the local server.

### When another window has the thread

A Herdr pane takes input from one live client at a time. Opening a thread in
Herdr Inbox takes it over, sized to the inbox's terminal area. If another
client takes it back, the inbox says so, and `Enter` brings it back.

## Develop

```sh
cargo test                   # unit tests and the installer, no Herdr needed
HERDR_E2E=1 cargo test --test e2e   # end to end against a real herdr
```

Release Please maintains a release PR from Conventional Commits on `main`,
updating `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md`. The maintainer merges
that PR to release. The workflow builds and tests all four binaries on
self-hosted runners, uploads their checksums, then publishes the draft release.
Linux releases cross-compile with native Rust and LLVM, then check the target
architecture and version without running the compiler under emulation.
No hand-written version bump or tag push is needed.

[CI and releases](docs/ci.md) covers the Blueprint Linux runners, the native
macOS runner, fork safety and retrying a failed release.

The end-to-end tests start their own Herdr server with every directory in a
temporary folder, so they never touch your sessions. They wait for a successful
API ping before creating workspaces. CI runs them against the Herdr release
pinned in `.github/workflows/ci.yml`; description-only PR edits do not consume
runners or cancel those checks. Each check runs once per push: the release
workflow builds binaries only on PRs that change how they are built.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

## License

[MIT](LICENSE). The built-in theme colours come from Herdr (Apache 2.0).
