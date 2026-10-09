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
since the Actions runner itself is a Docker container. The x86_64 build uses
the existing VM's emulation; ARM64 builds natively. Both binaries use musl and
run their version check inside the build container.

The native macOS runner lives at
`~/goinfre/actions-runners/herdr-inbox-macos`. Its background LaunchAgent is
`fr.scariot.herdr-inbox-runner` in the `user/501` domain, with its plist in
`~/Library/LaunchAgents/`. It runs without a GUI login, uses Xcode and Rosetta,
and restarts after an unexpected exit. Its HOME, Rust toolchains and config
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
The default workflow token is read-only; only Release Please and publication
get write permissions. Workflow-created PRs are enabled for Release Please.

Both PR workflows use `pull_request_target`, so GitHub reads their definitions
from the trusted base branch. Every job that can run PR code checks that the
head repository is this repository before checkout. Checkout pins the head SHA
and does not persist credentials. Fork PRs skip these jobs, even if someone
approves their workflow run. Never replace this with an unguarded
`pull_request` workflow on the self-hosted pool.

To test an external contribution, review its code and workflow changes first,
then copy the reviewed commit to a branch in this repository and open a PR.
A title check keeps squash commits in Conventional Commit format.

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
no job connects to personal sessions. Herdr is pinned in `ci.yml`.

The release workflow builds these unchanged installer asset names:

- `herdr-inbox-linux-x86_64.tar.gz`
- `herdr-inbox-linux-aarch64.tar.gz`
- `herdr-inbox-macos-x86_64.tar.gz`
- `herdr-inbox-macos-aarch64.tar.gz`

Each archive has an adjacent `.sha256` file and contains only `herdr-inbox`,
`LICENSE` and `README.md`. Packaging tests cover names, layout, executable
permissions, checksums and invalid inputs.

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

This preserves the successful Release Please job's outputs and release SHA.
Rerunning the entire workflow may no longer report `release_created`, so it is
not the publication retry path. Uploaded draft assets can be replaced by the
retry; the public previous release stays usable until publication succeeds.
