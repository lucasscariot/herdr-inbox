# Contributing

Herdr Inbox is a Rust binary (`src/`) that talks to a running Herdr server.

## Build and run

```sh
cargo run -- --session inbox-test   # against an isolated Herdr session
```

Start the isolated session with `herdr --session inbox-test server &` and
stop it with `herdr session stop inbox-test`. Anything you open, archive or
type there stays away from your real agents.

## Test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
HERDR_E2E=1 cargo test --test e2e   # needs herdr on PATH, or HERDR_BIN
```

- Behaviour lives in `App::update`, which does no I/O. Add a test in
  `src/app/tests.rs` for any behaviour you change.
- Drawing is tested on a `TestBackend`: text snapshots with `insta`
  (`cargo insta review` after an intended change) plus explicit checks on the
  colours that carry meaning.
- Protocol code is tested against a fake Herdr server on a real Unix socket
  (`src/testing.rs`), which keeps Herdr's connection rules: one request per
  connection, long-lived subscriptions. Subprocess fixtures report readiness
  before tests release or stop them, rather than relying on short startup sleeps.
- The end-to-end test starts a real Herdr server with every directory in a
  temporary folder and drives the real binary in a pseudo-terminal.
- Typing latency has two checks: `runtime::redraw` replays keys and echoes
  with a controlled clock, including the 16 ms frame limit; the PTY tests
  measure cursor movement after each key, including a paused space and
  continuous agent output. Their 250 ms deadline catches the old 700 ms
  delay without relying on sub-frame wall-clock timing on shared CI runners.
  Run `HERDR_E2E=1 cargo test --test e2e -- --nocapture` to see the samples.

The checks run on the self-hosted Blueprint Linux runners and a dedicated
macOS runner on Mac Studio. The release workflow also tests all four packaged
binaries. Fork PRs never run code on these machines; after review, a maintainer
can move the changes to a branch in this repository. See [CI and releases](docs/ci.md).

## Style

- No `unwrap()` outside tests.
- Keep render pure: `ui::draw` reads `&App` only.
- Herdr changes between releases: every type tolerates unknown fields, every
  enum has a fallback, and nothing depends on Herdr's private protocol.

## Commits and pull requests

Work on a branch in its own worktree, never on `main`. Commit messages and
pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
(`feat(composer): paste screenshots`); pull requests are squash-merged with
their title. [AGENTS.md](AGENTS.md) has the full rules that coding agents
follow here.
