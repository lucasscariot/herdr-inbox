//! The release archives must keep the names and layout used by the installer.
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn package(binary: &Path, asset: &str, output: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/package-release.sh"))
        .arg(binary)
        .arg(asset)
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn each_platform_has_a_flat_archive_and_a_portable_checksum() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("a binary");
    fs::write(&binary, "#!/bin/sh\necho herdr-inbox 1.0.0\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let output = temp.path().join("release assets");
    for asset in ["linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64"] {
        let result = package(&binary, asset, &output);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let archive = format!("herdr-inbox-{asset}.tar.gz");
        let listing = Command::new("tar").args(["-tzf"]).arg(output.join(&archive)).output().unwrap();
        assert!(listing.status.success());
        assert_eq!(String::from_utf8_lossy(&listing.stdout), "herdr-inbox\nLICENSE\nREADME.md\n");
        let checksum = fs::read_to_string(output.join(format!("{archive}.sha256"))).unwrap();
        let fields: Vec<_> = checksum.split_whitespace().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].len(), 64);
        assert!(fields[0].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(fields[1], archive);
        let mut checker = if Command::new("sha256sum").arg("--version").output().is_ok() {
            let mut command = Command::new("sha256sum");
            command.arg("--check");
            command
        } else {
            let mut command = Command::new("shasum");
            command.args(["-a", "256", "--check"]);
            command
        };
        let check = checker.current_dir(&output).arg(format!("{archive}.sha256")).output().unwrap();
        assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
        let extracted = temp.path().join(asset);
        fs::create_dir(&extracted).unwrap();
        let extraction =
            Command::new("tar").arg("-xzf").arg(output.join(&archive)).arg("-C").arg(&extracted).status().unwrap();
        assert!(extraction.success());
        assert_eq!(fs::read(extracted.join("herdr-inbox")).unwrap(), fs::read(&binary).unwrap());
        assert_ne!(fs::metadata(extracted.join("herdr-inbox")).unwrap().permissions().mode() & 0o111, 0);
    }
}

#[test]
fn an_unknown_platform_does_not_create_assets() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("assets");
    let result = package(Path::new("unused"), "../unexpected", &output);
    assert_eq!(result.status.code(), Some(2));
    assert!(!output.exists());
}

#[test]
fn a_missing_or_non_executable_binary_does_not_create_assets() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("binary");
    let output = temp.path().join("assets");
    assert!(!package(&binary, "linux-x86_64", &output).status.success());
    fs::write(&binary, "not executable").unwrap();
    assert!(!package(&binary, "linux-x86_64", &output).status.success());
    assert!(!output.exists());
}
