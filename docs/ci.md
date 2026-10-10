# CI and releases

## Runners

Inbox follows the Blueprint's self-hosted runner setup without calling its
private reusable workflows from this public repository. It uses the same
`blueprint` topic and Linux runner labels. No Cloud Run deployment, shared
infrastructure change or new cloud resources are involved.

Verified on 2026-10-09:

| Jobs | Runner labels | Location |
| --- | --- | --- |
| Linux checks, Release Please and Linux binaries | `self-hosted`, `Linux`, `ARM64`, `blueprint` | Three Docker runners on Mac Studio, managed by the Blueprint keeper |
| macOS checks and both macOS binaries | `self-hosted`, `macOS`, `ARM64`, `herdr-inbox` | One native runner on Mac Studio |

The keeper enrolls repositories with the `blueprint` topic within five minutes.
Linux release builds use BuildKit and the official Rust Alpine image. The
source travels through the build context, not a bind mount into the daemon,
since the Actions runner itself is a Docker container. The compiler runs on
`BUILDPLATFORM`, installs the requested musl target, and cross-links the final
binary with Rust's native `rust-lld`. Host proc macros keep their native `cc`.
This avoids the emulated x86 GCC `collect2` that segfaulted during the 1.0.0
release build. A separate target-platform stage checks the ELF architecture
and runs the version check. Only that brief x86_64 check needs emulation.
The shared VM has two CPUs and 4 GiB RAM, so CI disables test debug info and
uses two compiler workers.
Release builds wait for checks and run one at a time to avoid overlapping LLVM
linking. The VM's settings and other projects are unchanged.

The native macOS runner lives at
`~/goinfre/actions-runners/herdr-inbox-macos`. Its background LaunchAgent is
`fr.scariot.herdr-inbox-runner` in the `user/501` domain, with its plist in
`~/Library/LaunchAgents/`. It runs without a GUI login, uses Xcode and Rosetta,
and restarts after an unexpected exit. It uses the SDK's `runsvc.sh`,
`ACTIONS_RUNNER_SVC=1`, `SessionCreate=true` and `ProcessType=Interactive`, as in
GitHub's official service template, while keeping the headless Background
launch domain. Background and Standard process scheduling stretched 25 ms
keypresses beyond 150 ms and caused intermittent hold-to-dictate failures.
Its HOME, Rust toolchains and config
live inside the runner directory, separate from personal development settings.
Logs are in the runner's `logs/` and `_diag/` directories. A full reboot has not
been tested.

Check registration from any authenticated machine:

```sh
gh api repos/lucasscariot/herdr-inbox/actions/runners \
  --jq '.runners[] | {name, status, busy, labels: [.labels[].name]}'
```

Check the native service on Mac Studio:

```sh
launchctl print "user/$(id -u)/fr.scariot.herdr-inbox-runner"
```

These are persistent runners, not disposable sandboxes. The Linux runners
hold the Docker socket, and native jobs run as the Mac's user. Keep runner
credentials and personal tokens out of repository files and workflow artifacts.

## Fork safety

Repository Actions settings require approval for all external contributors.
The default workflow token is read-only. Only Release Please and publication
can write contents; result-reporting jobs can write commit statuses without
checking out or executing source. Workflow-created PRs are enabled for Release
Please.

Both PR workflows use `pull_request_target`, so GitHub reads their definitions
from the trusted base branch. Every job that can run PR code checks that the
head repository is this repository before checkout. Checkout pins the head SHA
and does not persist credentials. Fork PRs skip these jobs. Do not approve
fork-originated workflow runs: a fork could add a different workflow that
requests the same runners. The approval policy is the guard against those
new definitions. Never replace the trusted PR workflows with an unguarded
`pull_request` workflow on the self-hosted pool.

To test an external contribution, review its code and workflow changes first,
then copy the reviewed commit to a branch in this repository and open a PR.
A title check keeps squash commits in Conventional Commit format. Since
`pull_request_target` check runs attach to the base commit, the workflows also
report their actual results as commit statuses on the exact tested head SHA.
The same reporting makes build-only dispatches visible in the PR's checks.

## Release Please

Pushes to `main` update the release PR with the Rust strategy. It manages
`Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and `.release-please-manifest.json`,
and keeps tags in the installer's `vX.Y.Z` format.

The manifest starts at the last published plugin release, `0.4.0`. The setup
commit marks the switch to standalone Rust release archives as a breaking
change, so the first release PR proposes `1.0.0`. Subsequent bumps follow
Conventional Commits without a permanent version override. The bootstrap SHA
limits initial notes to commits after `v0.4.0`. Existing handwritten changelog
history stays in the file.

Release Please uses `GITHUB_TOKEN`, not a personal access token. Its PR and tag
events do not trigger other workflows, so the release job explicitly dispatches
a build-only run for its release PR. After that PR is merged, the same workflow
creates the tagged draft and runs checks and builds from the release SHA.

Nothing releases merely because an ordinary feature PR merges. The maintainer
must merge the separate `chore(main): release ...` PR. Do not manually push a
tag or bump versions alongside Release Please.

## Builds and publication

CI runs formatting, Clippy, unit and integration tests, ShellCheck, and the real
Herdr end-to-end tests on Linux and macOS. Each test starts isolated Herdr state;
no job connects to personal sessions. Server startup waits for an API ping,
not the appearance of a socket file; early exits include the status and stderr.
The update dialog checks its real compiled version before normalizing that
field for snapshots, so Release Please's version bumps do not break layout tests. The PTY tests run serially so concurrent
TUIs do not distort their input timing. Their 250 ms latency limit is unchanged.
Herdr is pinned in `ci.yml`. Subprocess
fixtures wait for readiness before release or SIGINT rather than assuming a
short startup time on a busy self-hosted Mac.

Description-only PR edits run no jobs and use a separate concurrency group,
so editing the PR body neither cancels code checks nor replaces a pending run.
GitHub still records the skipped workflow event. Code updates, title edits and
base changes still run the checks; obsolete code runs cancel as before.
`tests/workflows.rs` locks down these guards and the existing fork restrictions.

The release workflow builds these unchanged installer asset names:

- `herdr-inbox-linux-x86_64.tar.gz`
- `herdr-inbox-linux-aarch64.tar.gz`
- `herdr-inbox-macos-x86_64.tar.gz`
- `herdr-inbox-macos-aarch64.tar.gz`

Each archive has an adjacent `.sha256` file and contains only `herdr-inbox`,
`LICENSE` and `README.md`. Packaging tests cover names, layout, executable
permissions, checksums and invalid inputs. The packager normalizes the binary
to mode 755 and documents to 644, even with the native runner's private umask.

Publication waits for CI and all four builds, checks that the tag matches the
crate version, verifies every archive's checksum, uploads all eight assets,
then publishes the draft. Until then, the public installer and Ctrl+G keep
seeing the preceding stable release. Publication refuses to overwrite a release
that is already public.

For a build-only check of a trusted branch:

```sh
gh workflow run release.yml --ref <branch>
```

Manual dispatch only builds and tests. It never runs Release Please, creates a
tag, uploads release assets or publishes a release. This is also how to verify
workflow changes before the new `pull_request_target` definitions reach `main`.

If a real release fails, leave its draft alone and rerun only the failed jobs:

```sh
gh run rerun <run-id> --failed
```

This can publish the existing draft. It preserves the successful Release
Please job's outputs and release SHA, using the original workflow and source.
A later CI fix does not change that old run's build instructions.
Rerunning the entire workflow may no longer report `release_created`, so it is
not the publication retry path. Uploaded draft assets can be replaced by the
retry; the public previous release stays usable until publication succeeds.
