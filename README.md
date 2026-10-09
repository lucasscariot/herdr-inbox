# Herdr Inbox

A dedicated terminal client for [Herdr](https://herdr.dev), built for people
who run many coding agents across many projects. Every agent thread sits in
one list, sorted by what it needs from you. The thread you pick runs live next
to it, fully interactive.

```
 herdr inbox                        │
 1 needs input · 1 ready            │ ✻ Claude Code
                                    │
 NEEDS INPUT  1                     │ > Fix the login redirect loop on mobile
▎cockpit                    ● input │
▎Fix the login redirect loop        │   Do you want to make this edit?
▎⎇ fix-login-redirect · Claude      │   ❯ 1. Yes
                                    │     2. No
 READY  1                           │
▎site                       ✓ ready │
▎Review navigation                  │
▎⎇ main · OpenCode               2m │
                                    │
 WORKING  1                         │
▎api                      ◐ working │
▎Add invoice export                 │
▎⎇ main · Codex                 14s │
  AGENT  tab threads
```

Herdr Inbox is its own program and leaves your Herdr setup alone. It talks to
the Herdr server you already run, through Herdr's public interfaces only:

- the JSON socket API for threads, status and archiving;
- `herdr terminal session control` for the live terminal, Herdr's documented
  interface for third-party clients.

It never replaces `herdr`, never starts a server unless you ask it to, and
never updates anything. Plain `herdr` keeps working beside it, on the same
sessions.

> **Status.** The standalone client is being rebuilt in stages. It already
> covers threads on this machine and on every saved SSH machine: the live list,
> the interactive terminal, and archiving. The composer for new threads and
> dictation follow. The original Python plugin lives in [`legacy/`](legacy/)
> until then.

## Install

You need Herdr 0.9.2 or newer, and Rust 1.88 or newer to build.

```sh
cargo install --locked --git https://github.com/lucasscariot/herdr-inbox
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
| Threads | `j` `k` / `↓` `↑` | Move |
| | `g` `G` / `Home` `End` | First / last |
| | `Enter`, `o`, click | Open the thread and focus its agent |
| | `x`, `Delete`, `Backspace` | Archive: closes the thread's workspace, keeps its worktree on disk |
| | `Tab`, `Esc` | Back to the agent |
| | `q`, `Ctrl+C` | Quit |
| Agent | anything | Goes to the agent, including `Ctrl+C` and `Shift+Tab` |
| | `Tab` | Back to the threads |

Threads are grouped as **needs input**, **ready** (finished, not yet looked
at), **working** and **idle**. The most recent change in each group comes first.

Herdr Inbox uses your Herdr theme: the same built-in themes, `[theme] name`,
and `[theme.custom]` colours from `~/.config/herdr/config.toml`.

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
cargo test                   # unit tests, no Herdr needed
HERDR_E2E=1 cargo test --test e2e   # end to end against a real herdr
```

The end-to-end tests start their own Herdr server with every directory in a
temporary folder, so they never touch your sessions. CI runs them against the
Herdr release pinned in `.github/workflows/ci.yml`.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

## License

[MIT](LICENSE). The built-in theme colours come from Herdr (Apache 2.0).
