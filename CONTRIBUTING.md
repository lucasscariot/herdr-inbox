# Contributing

Thanks for helping. Herdr Inbox is plain Python 3.9+ with the standard library
only; keep it that way so it installs with a clone and runs its discovery and
event relay on remote machines without installing anything there.

## Run it

```sh
herdr plugin link "$PWD"            # register this checkout with your Herdr
python3 main.py ui --demo           # composer preview, no Herdr needed
python3 main.py ui --demo --view inbox
```

After editing, close an open composer with Escape and reopen it; Herdr starts
the pane command fresh each time. `F5` hot-reloads `inbox/banner.py`.

## Test it

```sh
python3 -m unittest discover -s tests -v
```

The suite runs in a few seconds and needs no Herdr. It includes a fake Herdr
socket server (`tests/test_live.py`) that mimics the real subscription rules,
so changes to `inbox/relay.py` and `inbox/live.py` can be verified offline.
CI runs the suite on Linux and macOS with Python 3.9 and 3.13.

## Validate against a real server without touching your session

Start an isolated Herdr server and point the plugin at it:

```sh
herdr --session inbox-test server &
export HERDR_SOCKET_PATH=~/.config/herdr/sessions/inbox-test/herdr.sock HERDR_ENV=1
export HERDR_PLUGIN_STATE_DIR=/tmp/inbox-state HERDR_PLUGIN_CONFIG_DIR=/tmp/inbox-config
python3 main.py launch --machine Local --project <name> --harness claude --worktree --task 'Reply with one word: ok'
python3 main.py list
herdr session stop inbox-test && herdr session delete inbox-test
```

Worktrees Herdr creates during such a run live under `~/.herdr/worktrees/`;
remove them with `git worktree remove` when done.

## Style

- No third-party dependencies, no build step.
- Keep `inbox/relay.py` self-contained: it is shipped to remote hosts as source.
- Every launch step journals before it mutates anything; keep that invariant.
- Prefer small, named functions over flags; keep docstrings on modules and on
  anything with a non-obvious contract.
- Add or update a test for behaviour you change. Bump `version` in
  `herdr-plugin.toml` and add a `CHANGELOG.md` entry for user-visible changes.

## Reporting bugs

Include `herdr status`, the plugin version, the machine kind (Local or saved
SSH), and the relevant lines from `herdr plugin log list --plugin
lucas.herdr-inbox`. Launch journals under the plugin state directory
(`threads/*.json`) contain the exact stage a launch reached.
