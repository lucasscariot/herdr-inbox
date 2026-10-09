# Agent instructions

These rules apply to every coding agent working in this repository (Claude
Code, Codex, Pi, opencode and others). [CONTRIBUTING.md](CONTRIBUTING.md) has
the build and test commands; [ARCHITECTURE.md](ARCHITECTURE.md) explains the
code.

## Every task ends with a pull request

Treat each task as a full, end-to-end implementation. It is done only when a
pull request is open on GitHub, its checks pass, and it is ready to merge. A
local diff, a plan or a branch without a pull request is not done.

1. Start in a new worktree from an up-to-date `origin/main` (see below).
2. Implement the change, with tests for any behaviour you add or change.
3. Run every check in [CONTRIBUTING.md](CONTRIBUTING.md#test): `cargo fmt
   --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and
   `HERDR_E2E=1 cargo test --test e2e` when Herdr is installed.
4. Try the change in the real binary against an isolated Herdr session
   (`herdr --session inbox-test`), never the user's live session.
5. Update `README.md`, `CHANGELOG.md` and `ARCHITECTURE.md` when what they
   describe changes.
6. Commit, push, and open the pull request with `gh pr create`.
7. Wait for CI. If a check fails, fix it and push again until all are green.

Do not merge the pull request and do not push release tags: those are the
maintainer's call.

Stop and ask only for a real product decision the code and the request cannot
settle. Otherwise make the sensible choice, finish, and list the choices you
made in the pull request description.

## Work in a new worktree

Never work in the main checkout or on `main`. Each task gets its own git
worktree on its own branch, created from the latest `origin/main`:

```sh
git fetch origin
git worktree add -b <branch> ~/.herdr/worktrees/herdr-inbox/<branch> origin/main
```

If the session already starts in a fresh worktree made for this task (Herdr
and Herdr Inbox create one per thread), use it, but first make sure its
branch starts from the latest `origin/main`: a local `main` can be behind.
Name the branch after the change (`paste-images`, `fix-archive-notice`),
renaming it with `git branch -m` if needed.

## Conventional Commits

Commit messages and pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<optional scope>): <summary in the imperative, lower case>
```

- Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`,
  `chore`, `revert`.
- Scope, when it helps: `composer`, `threads`, `dictation`, `launch`,
  `machines`, `ui`, `install`...
- A breaking change adds `!` after the type and a `BREAKING CHANGE:` footer.
- Examples: `feat(composer): paste screenshots as attachments`,
  `fix(launch): keep the task when the trust dialog reappears`.

Pull requests are squash-merged with their title as the commit message, so
the title must follow the same format. The description says what changed and
why, the trade-offs chosen without asking, and how the change was verified.
