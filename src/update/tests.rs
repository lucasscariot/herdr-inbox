use super::*;
use crate::testing::write_executable;
use serde_json::json;

fn github(tag: &str, assets: bool) -> Vec<u8> {
    let names = if assets {
        let name = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        vec![json!({"name": name}), json!({"name": format!("{name}.sha256")})]
    } else {
        vec![]
    };
    serde_json::to_vec(&json!({
        "tag_name": tag, "body": "## Changes\nA new inbox.", "assets": names,
        "draft": false, "prerelease": false, "unknown": "ignored"
    }))
    .unwrap()
}

fn release(tag: &str) -> Release {
    parse_release(&github(tag, true), "1.0.0", std::env::consts::OS, std::env::consts::ARCH).unwrap()
}

#[test]
fn a_stable_release_needs_both_native_assets_to_be_installable() {
    let release = release("v1.1.0");
    assert!(release.can_install());
    assert_eq!(release.url(), format!("{RELEASES_URL}/tag/v1.1.0"));
    assert_eq!(release.notes, "## Changes\nA new inbox.");
    let mut response: serde_json::Value = serde_json::from_slice(&github("v1.1.0", true)).unwrap();
    response["assets"].as_array_mut().unwrap().pop();
    let release =
        parse_release(&serde_json::to_vec(&response).unwrap(), "1.0.0", std::env::consts::OS, std::env::consts::ARCH)
            .unwrap();
    assert!(release.newer);
    assert!(!release.can_install(), "an archive without a checksum cannot be installed");
}

#[test]
fn versions_compare_semantically_and_never_downgrade() {
    for (current, latest, newer) in [
        ("1.9.0", "v1.10.0", true),
        ("1.10.0", "v1.9.0", false),
        ("1.0.0", "v1.0.0", false),
        ("1.0.0", "v0.4.0", false),
        ("1.0.0-rc.1", "v1.0.0", true),
        ("1.0.0", "v1.0.0+build.2", false),
    ] {
        let release =
            parse_release(&github(latest, true), current, std::env::consts::OS, std::env::consts::ARCH).unwrap();
        assert_eq!(release.can_install(), newer, "{current} -> {latest}");
    }
}

#[test]
fn malformed_draft_and_prerelease_responses_are_rejected() {
    assert!(parse_release(b"not JSON", "1.0.0", "linux", "x86_64").unwrap_err().contains("invalid release"));
    for tag in ["main", "vnext", "v1.0.1;echo hacked", "v2.0.0-rc.1"] {
        assert!(parse_release(&github(tag, true), "1.0.0", "linux", "x86_64").is_err(), "{tag}");
    }
    for field in ["draft", "prerelease"] {
        let mut response: serde_json::Value = serde_json::from_slice(&github("v2.0.0", true)).unwrap();
        response[field] = true.into();
        assert!(parse_release(&serde_json::to_vec(&response).unwrap(), "1.0.0", "linux", "x86_64").is_err());
    }
}

#[test]
fn old_releases_and_unsupported_platforms_can_still_be_browsed() {
    let release = parse_release(&github("v0.4.0", false), "1.0.0", "linux", "x86_64").unwrap();
    assert!(!release.newer);
    assert!(release.asset.is_none());
    let release = parse_release(&github("v2.0.0", true), "1.0.0", "linux", "riscv64").unwrap();
    assert!(release.newer);
    assert!(!release.can_install());
    assert_eq!(asset_name("macos", "aarch64").as_deref(), Some("herdr-inbox-macos-aarch64.tar.gz"));
    assert!(asset_name("windows", "x86_64").is_none());
}

#[test]
fn missing_notes_are_allowed_and_control_sequences_are_removed() {
    let response = json!({"tag_name": "v2.0.0", "body": null});
    let release = parse_release(&serde_json::to_vec(&response).unwrap(), "1.0.0", "linux", "x86_64").unwrap();
    assert!(release.notes.is_empty());
    let response = json!({"tag_name": "v2.0.0", "body": "hello\x1b[2J\r\nworld\x07"});
    let release = parse_release(&serde_json::to_vec(&response).unwrap(), "1.0.0", "linux", "x86_64").unwrap();
    assert_eq!(release.notes, "hello[2J\nworld");
}

#[test]
fn checks_use_the_stable_endpoint_and_bounded_noninteractive_curl() {
    let dir = tempfile::tempdir().unwrap();
    let curl = dir.path().join("curl");
    let args = dir.path().join("args");
    let response = dir.path().join("release.json");
    std::fs::write(&response, github("v1.1.0", true)).unwrap();
    write_executable(
        &curl,
        &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\n", args.display(), response.display()),
    );
    assert!(check_with(&curl, "1.0.0", std::env::consts::OS, std::env::consts::ARCH).unwrap().can_install());
    let args = std::fs::read_to_string(args).unwrap();
    assert!(args.starts_with("-q\n"), "ignore user curl config");
    assert!(args.contains("--connect-timeout\n10\n--max-time\n30\n"));
    assert!(args.ends_with(&format!("{LATEST_URL}\n")));
    assert!(!args.contains("Authorization"), "no GitHub credentials are needed");
}

#[test]
fn network_failure_and_a_missing_curl_are_actionable_errors() {
    let dir = tempfile::tempdir().unwrap();
    let curl = dir.path().join("curl");
    write_executable(&curl, "#!/bin/sh\necho 'HTTP 403: rate limit exceeded' >&2\nexit 22\n");
    let error = check_with(&curl, "1.0.0", "linux", "x86_64").unwrap_err();
    assert!(error.contains("rate limit exceeded") && error.contains("retry"), "{error}");
    assert!(
        check_with(&dir.path().join("missing"), "1.0.0", "linux", "x86_64").unwrap_err().contains("Cannot run curl")
    );
}

/// Native release files served by a fake curl. The actual embedded installer
/// still extracts, verifies and atomically replaces the binary.
fn fixture(dir: &Path, version: &str) -> (PathBuf, PathBuf, PathBuf) {
    let stage = dir.join("stage");
    let tools = dir.join("tools");
    let bin = dir.join("custom-install/bin");
    for path in [&stage, &tools, &bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    write_executable(&stage.join("herdr-inbox"), &format!("#!/bin/sh\necho 'herdr-inbox {version}'\n"));
    let name = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let archive = dir.join(&name);
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("herdr-inbox")
            .status()
            .unwrap()
            .success()
    );
    let digest = Command::new("sh")
        .args(["-c", "if command -v sha256sum >/dev/null; then sha256sum \"$1\"; else shasum -a 256 \"$1\"; fi", "sh"])
        .arg(&archive)
        .output()
        .unwrap();
    let hash = String::from_utf8_lossy(&digest.stdout).split_whitespace().next().unwrap().to_string();
    std::fs::write(dir.join(format!("{name}.sha256")), format!("{hash}  {name}\n")).unwrap();
    let curl = format!(
        "#!/bin/sh\nurl=\nout=\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    -o) out=\"$2\"; shift 2;;\n    https://*) url=\"$1\"; shift;;\n    *) shift;;\n  esac\ndone\ncase \"$url\" in\n  https://github.com/{REPOSITORY}/releases/download/v2.0.0/{name}*) cp '{}/'\"${{url##*/}}\" \"$out\";;\n  *) echo \"unexpected URL: $url\" >&2; exit 1;;\nesac\n",
        dir.display()
    );
    write_executable(&tools.join("curl"), &curl);
    let shell = tools.join("installer-sh");
    let log = dir.join("installer.env");
    write_executable(
        &shell,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$HERDR_INBOX_REPO\" \"$HERDR_INBOX_VERSION\" \"$HERDR_INBOX_EXPECTED_VERSION\" \"$HERDR_INBOX_INSTALL_DIR\" \"$HERDR_INBOX_BASE_URL\" > '{}'\nPATH='{}':\"$PATH\" exec sh \"$@\"\n",
            log.display(),
            tools.display()
        ),
    );
    let binary = bin.join("herdr-inbox");
    write_executable(&binary, "#!/bin/sh\necho 'herdr-inbox 1.0.0'\n");
    (binary, shell, archive)
}

#[test]
fn installation_pins_the_reviewed_release_and_replaces_only_the_running_path() {
    let dir = tempfile::tempdir().unwrap();
    let (binary, shell, _) = fixture(dir.path(), "2.0.0");
    let herdr = dir.path().join("custom-install/bin/herdr");
    std::fs::write(&herdr, "untouched").unwrap();
    let installed = install_at(&release("v2.0.0"), &binary, &shell).unwrap();
    assert_eq!(installed.path, std::fs::canonicalize(&binary).unwrap());
    assert_eq!(installed.version, Version::new(2, 0, 0));
    let output = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "herdr-inbox 2.0.0");
    assert_eq!(std::fs::read_to_string(&herdr).unwrap(), "untouched");
    let log = std::fs::read_to_string(dir.path().join("installer.env")).unwrap();
    let directory = installed.path.parent().unwrap().display().to_string();
    assert_eq!(log, format!("{REPOSITORY}\nv2.0.0\n2.0.0\n{directory}\n\n"));
    assert!(
        std::fs::read_dir(binary.parent().unwrap()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".herdr-inbox"))
    );
}

#[test]
fn checksum_and_version_failures_keep_the_old_binary() {
    for wrong_version in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let version = if wrong_version { "1.5.0" } else { "2.0.0" };
        let (binary, shell, archive) = fixture(dir.path(), version);
        if !wrong_version {
            std::fs::write(archive.with_extension("gz.sha256"), "0000\n").unwrap();
        }
        let old = std::fs::read(&binary).unwrap();
        let error = install_at(&release("v2.0.0"), &binary, &shell).unwrap_err();
        assert!(error.contains(if wrong_version { "version does not match" } else { "checksum mismatch" }), "{error}");
        assert_eq!(std::fs::read(&binary).unwrap(), old);
        assert!(
            std::fs::read_dir(binary.parent().unwrap()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".herdr-inbox"))
        );
    }
}

#[test]
fn a_renamed_binary_or_an_older_release_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("renamed-inbox");
    std::fs::write(&binary, "old").unwrap();
    assert!(install_at(&release("v2.0.0"), &binary, Path::new("missing-sh")).unwrap_err().contains("renamed"));
    assert!(install_at(&release("v0.4.0"), &binary, Path::new("missing-sh")).unwrap_err().contains("no newer binary"));
    assert_eq!(std::fs::read_to_string(binary).unwrap(), "old");
}
