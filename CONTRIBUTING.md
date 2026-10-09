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
  connection, long-lived subscriptions.
- The end-to-end test starts a real Herdr server with every directory in a
  temporary folder and drives the real binary in a pseudo-terminal.

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
