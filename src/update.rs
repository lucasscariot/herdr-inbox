//! Self-updates from stable GitHub releases. The running binary is replaced
//! only on request, through the same checksum-checked installer as install.sh.
//! Herdr, configuration and agent sessions are never changed here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;
use serde::Deserialize;

pub const REPOSITORY: &str = "lucasscariot/herdr-inbox";
pub const RELEASES_URL: &str = "https://github.com/lucasscariot/herdr-inbox/releases";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_URL: &str = "https://api.github.com/repos/lucasscariot/herdr-inbox/releases/latest";
const INSTALLER: &str = include_str!("../install.sh");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub notes: String,
    pub newer: bool,
    /// Both this platform's archive and its checksum were published.
    pub asset: Option<String>,
}

impl Release {
    pub fn url(&self) -> String {
        format!("{RELEASES_URL}/tag/{}", self.tag)
    }

    pub fn can_install(&self) -> bool {
        self.newer && self.asset.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: Version,
    pub path: PathBuf,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
}

pub fn asset_name(os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("linux" | "macos", "x86_64" | "aarch64") => Some(format!("herdr-inbox-{os}-{arch}.tar.gz")),
        _ => None,
    }
}

fn parse_release(bytes: &[u8], current: &str, os: &str, arch: &str) -> Result<Release, String> {
    let release: GithubRelease =
        serde_json::from_slice(bytes).map_err(|e| format!("GitHub returned an invalid release: {e}"))?;
    let version = Version::parse(release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name))
        .map_err(|e| format!("GitHub's release tag is not a version: {e}"))?;
    if release.draft || release.prerelease || !version.pre.is_empty() {
        return Err("GitHub did not return a stable release. Press r to check again later.".into());
    }
    let current = Version::parse(current).map_err(|e| format!("Cannot compare the running version: {e}"))?;
    let asset = asset_name(os, arch).filter(|name| {
        release.assets.iter().any(|a| &a.name == name)
            && release.assets.iter().any(|a| a.name == format!("{name}.sha256"))
    });
    // Release notes are remote text, not terminal control sequences.
    let notes = release.body.unwrap_or_default().chars().filter(|c| !c.is_control() || *c == '\n').collect();
    Ok(Release { tag: release.tag_name, newer: version.cmp_precedence(&current).is_gt(), version, notes, asset })
}

pub fn check() -> Result<Release, String> {
    check_with(Path::new("curl"), CURRENT_VERSION, std::env::consts::OS, std::env::consts::ARCH)
}

fn check_with(curl: &Path, current: &str, os: &str, arch: &str) -> Result<Release, String> {
    let output = Command::new(curl)
        .args([
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--header",
            "Accept: application/vnd.github+json",
            "--user-agent",
            concat!("herdr-inbox/", env!("CARGO_PKG_VERSION")),
            LATEST_URL,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Cannot run curl to check GitHub: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not check GitHub: {}. Press r to retry.", detail.trim()));
    }
    parse_release(&output.stdout, current, os, arch)
}

pub fn install(release: &Release) -> Result<Installed, String> {
    let executable = std::env::current_exe().map_err(|e| format!("Cannot locate the running Inbox: {e}"))?;
    install_at(release, &executable, Path::new("sh"))
}

fn install_at(release: &Release, executable: &Path, shell: &Path) -> Result<Installed, String> {
    if !release.can_install() {
        return Err("This release has no newer binary for this platform.".into());
    }
    let executable = std::fs::canonicalize(executable).map_err(|e| format!("Cannot locate the running Inbox: {e}"))?;
    if executable.file_name().is_none_or(|name| name != "herdr-inbox") {
        return Err("This binary was renamed. Use install.sh to update it manually.".into());
    }
    let directory = executable.parent().ok_or("The running Inbox has no install directory.")?;
    let output = Command::new(shell)
        .args(["-c", INSTALLER])
        // Pin the release the user reviewed, and the binary actually running,
        // not a default install directory or inherited installer overrides.
        .env("HERDR_INBOX_REPO", REPOSITORY)
        .env("HERDR_INBOX_VERSION", &release.tag)
        .env("HERDR_INBOX_EXPECTED_VERSION", release.version.to_string())
        .env("HERDR_INBOX_INSTALL_DIR", directory)
        .env("HERDR_INBOX_BASE_URL", "")
        .env("HERDR_INBOX_OS", std::env::consts::OS)
        .env("HERDR_INBOX_ARCH", std::env::consts::ARCH)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Cannot start the Inbox installer: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not update {}: {}", executable.display(), detail.trim()));
    }
    Ok(Installed { version: release.version.clone(), path: executable })
}

#[cfg(test)]
mod tests;
