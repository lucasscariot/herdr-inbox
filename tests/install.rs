//! The installer, run for real against a fake release on disk.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn installer() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

/// Writes an executable through a child `sh`, so no parallel test's fork can
/// hold it open for writing.
fn write_executable(path: &Path, body: &str) {
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("sh");
    child.stdin.take().expect("stdin").write_all(body.as_bytes()).expect("write");
    assert!(child.wait().expect("wait").success());
}

fn sha256(path: &Path) -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg("if command -v sha256sum >/dev/null; then sha256sum \"$1\"; else shasum -a 256 \"$1\"; fi")
        .arg("sh")
        .arg(path)
        .output()
        .expect("sha256");
    String::from_utf8_lossy(&output.stdout).split_whitespace().next().expect("digest").to_string()
}

/// A release directory with `herdr-inbox-<platform>.tar.gz` and its checksum.
fn release(dir: &Path, platform: &str, version: &str) -> PathBuf {
    let stage = dir.join(format!("stage-{platform}"));
    std::fs::create_dir_all(&stage).unwrap();
    write_executable(&stage.join("herdr-inbox"), &format!("#!/bin/sh\necho 'herdr-inbox {version}'\n"));
    let releases = dir.join("releases");
    std::fs::create_dir_all(&releases).unwrap();
    let asset = releases.join(format!("herdr-inbox-{platform}.tar.gz"));
    let status = Command::new("tar").arg("-czf").arg(&asset).arg("-C").arg(&stage).arg("herdr-inbox").status().unwrap();
    assert!(status.success());
    std::fs::write(asset.with_extension("gz.sha256"), format!("{}  herdr-inbox-{platform}.tar.gz\n", sha256(&asset)))
        .unwrap();
    releases
}

fn install(dir: &Path, releases: &Path, os: &str, arch: &str, path: &str) -> Output {
    Command::new("sh")
        .arg(installer())
        .env("HERDR_INBOX_BASE_URL", format!("file://{}", releases.display()))
        .env("HERDR_INBOX_INSTALL_DIR", dir.join("bin"))
        .env("HERDR_INBOX_OS", os)
        .env("HERDR_INBOX_ARCH", arch)
        .env("HOME", dir)
        .env("PATH", path)
        .output()
        .unwrap()
}

/// Everything the script said, for assertion messages.
fn said(output: &Output) -> String {
    format!(
        "status {:?}\n--- stdout\n{}--- stderr\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn system_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

#[test]
fn it_installs_the_binary_for_this_platform_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "linux-x86_64", "1.2.3");
    let path = format!("{}:{}", dir.path().join("bin").display(), system_path());
    let output = install(dir.path(), &releases, "Linux", "amd64", &path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", said(&output));
    assert!(stdout.contains("Downloading herdr-inbox-linux-x86_64.tar.gz"), "{stdout}");
    assert!(
        stdout.contains(&format!("Installed herdr-inbox 1.2.3 to {}/bin/herdr-inbox", dir.path().display())),
        "{stdout}"
    );
    assert!(!stdout.contains("not on your PATH"), "{stdout}");
    let binary = dir.path().join("bin/herdr-inbox");
    let version = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "herdr-inbox 1.2.3");
    assert!(!dir.path().join("bin/.herdr-inbox.new").exists(), "no temporary file is left");
}

#[test]
fn platform_names_are_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "macos-aarch64", "1.0.0");
    let output = install(dir.path(), &releases, "Darwin", "arm64", &system_path());
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn a_tampered_download_installs_nothing_and_keeps_the_old_binary() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "linux-x86_64", "2.0.0");
    std::fs::write(releases.join("herdr-inbox-linux-x86_64.tar.gz.sha256"), "0000  herdr-inbox-linux-x86_64.tar.gz\n")
        .unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/herdr-inbox"), "old").unwrap();
    let output = install(dir.path(), &releases, "Linux", "x86_64", &system_path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(dir.path().join("bin/herdr-inbox")).unwrap(), "old");
}

#[test]
fn an_unsupported_platform_points_to_building_from_source() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "linux-x86_64", "1.0.0");
    let output = install(dir.path(), &releases, "Linux", "riscv64", &system_path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no prebuilt binary for riscv64") && stderr.contains("cargo install --locked --git"),
        "{stderr}"
    );
    let output = install(dir.path(), &releases, "FreeBSD", "x86_64", &system_path());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no prebuilt binary for FreeBSD"));
}

#[test]
fn a_missing_release_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "linux-x86_64", "1.0.0");
    let output = install(dir.path(), &releases, "Linux", "aarch64", &system_path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not download"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.path().join("bin/herdr-inbox").exists());
}

#[test]
fn an_existing_binary_is_replaced_and_path_and_herdr_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let releases = release(dir.path(), "linux-x86_64", "3.0.0");
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/herdr-inbox"), "#!/bin/sh\necho old wrapper\n").unwrap();
    // A PATH with the system tools but neither the install dir nor herdr.
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    for tool in [
        "sh",
        "curl",
        "tar",
        "gzip",
        "mktemp",
        "cut",
        "cp",
        "mv",
        "chmod",
        "mkdir",
        "rm",
        "sha256sum",
        "shasum",
        "uname",
        "cat",
    ] {
        if let Ok(output) = Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output() {
            let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !found.is_empty() {
                let _ = std::os::unix::fs::symlink(&found, tools.join(tool));
            }
        }
    }
    let output = install(dir.path(), &releases, "Linux", "x86_64", &tools.display().to_string());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", said(&output));
    assert!(stdout.contains("is not on your PATH"), "{stdout}");
    assert!(stdout.contains("Herdr 0.9.2 or newer, which is not installed yet"), "{stdout}");
    let version = Command::new(dir.path().join("bin/herdr-inbox")).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "herdr-inbox 3.0.0");
}
