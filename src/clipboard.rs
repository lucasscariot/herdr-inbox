//! Reads the desktop clipboard for Ctrl+V in the composer and the reply box:
//! an image is saved to a private cache directory and its path returned, text
//! comes back as it is. `wl-paste` on Wayland, `xclip` on X11, `osascript`
//! and `pbpaste` on macOS.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::speech::recorder::which;

/// Image types agents accept, best first, with the file extension to save.
const IMAGE_TYPES: &[(&str, &str)] =
    &[("image/png", "png"), ("image/jpeg", "jpg"), ("image/gif", "gif"), ("image/webp", "webp")];
/// A clipboard owner that hangs must not hold the paste forever.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Saved images older than this are removed when a new one is saved.
const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    /// The path of the saved image.
    Image(String),
    Text(String),
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Wayland(PathBuf),
    X11(PathBuf),
    Mac,
}

/// The clipboard tool for this desktop, or why there is none.
pub fn tool() -> Result<Tool, String> {
    if cfg!(target_os = "macos") {
        return Ok(Tool::Mac);
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if set("WAYLAND_DISPLAY") {
        return which("wl-paste").map(Tool::Wayland).ok_or_else(|| "Install wl-clipboard to paste images.".into());
    }
    if set("DISPLAY") {
        return which("xclip").map(Tool::X11).ok_or_else(|| "Install xclip to paste images.".into());
    }
    Err("There is no desktop clipboard here.".into())
}

/// `$XDG_CACHE_HOME/herdr-inbox/images`, else the platform's cache directory.
/// Its path always contains [`crate::images::DIR_MARKER`].
pub fn image_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home_cache = if cfg!(target_os = "macos") { "Library/Caches" } else { ".cache" };
    var("XDG_CACHE_HOME")
        .or_else(|| var("HOME").map(|home| home.join(home_cache)))
        .map(|base| base.join("herdr-inbox").join("images"))
}

/// Reads the clipboard, saving an image into `dir`.
pub fn read(tool: &Tool, dir: &Path, now: SystemTime) -> Result<Clip, String> {
    match tool {
        Tool::Wayland(program) => {
            // wl-paste fails when nothing is copied.
            let Ok(types) = output(Command::new(program).arg("--list-types")) else {
                return Ok(Clip::Empty);
            };
            let types = String::from_utf8_lossy(&types);
            match pick(&types) {
                Some((mime, ext)) => {
                    let bytes = output(Command::new(program).args(["--no-newline", "--type", mime]))?;
                    save(dir, ext, &bytes, now).map(Clip::Image)
                }
                None if types.lines().any(is_text) => {
                    text(output(Command::new(program).args(["--no-newline", "--type", "text"])).unwrap_or_default())
                }
                None => Ok(Clip::Empty),
            }
        }
        Tool::X11(program) => {
            let clipboard = || {
                let mut command = Command::new(program);
                command.args(["-selection", "clipboard"]);
                command
            };
            let Ok(targets) = output(clipboard().args(["-t", "TARGETS", "-o"])) else {
                return Ok(Clip::Empty);
            };
            let targets = String::from_utf8_lossy(&targets);
            match pick(&targets) {
                Some((mime, ext)) => {
                    let bytes = output(clipboard().args(["-t", mime, "-o"]))?;
                    save(dir, ext, &bytes, now).map(Clip::Image)
                }
                None => text(output(clipboard().arg("-o")).unwrap_or_default()),
            }
        }
        Tool::Mac => {
            prepare(dir)?;
            let path = free_path(dir, "png", now);
            let script = [
                "on run argv",
                "set f to open for access (POSIX file (item 1 of argv)) with write permission",
                "try",
                "write (the clipboard as «class PNGf») to f",
                "on error message",
                "close access f",
                "error message",
                "end try",
                "close access f",
                "end run",
            ];
            let mut osascript = Command::new("osascript");
            for line in script {
                osascript.args(["-e", line]);
            }
            if output(osascript.arg(&path)).is_ok() && std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
                prune(dir, &path, now);
                return Ok(Clip::Image(path.to_string_lossy().into_owned()));
            }
            let _ = std::fs::remove_file(&path);
            text(output(&mut Command::new("pbpaste")).unwrap_or_default())
        }
    }
}

/// The best image type on offer.
fn pick(types: &str) -> Option<(&'static str, &'static str)> {
    IMAGE_TYPES.iter().copied().find(|(mime, _)| types.lines().any(|t| t.trim() == *mime))
}

fn is_text(mime: &str) -> bool {
    let mime = mime.trim();
    mime.starts_with("text/") || matches!(mime, "UTF8_STRING" | "STRING" | "TEXT")
}

fn text(bytes: Vec<u8>) -> Result<Clip, String> {
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if text.is_empty() { Clip::Empty } else { Clip::Text(text) })
}

/// A private directory: screenshots can hold anything.
fn prepare(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .and_then(|_| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)))
        .map_err(|err| format!("Could not save the image in {}: {err}", dir.display()))
}

fn free_path(dir: &Path, ext: &str, now: SystemTime) -> PathBuf {
    let millis = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
    let mut path = dir.join(format!("image-{millis}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("image-{millis}-{n}.{ext}"));
        n += 1;
    }
    path
}

fn save(dir: &Path, ext: &str, bytes: &[u8], now: SystemTime) -> Result<String, String> {
    use std::os::unix::fs::OpenOptionsExt;
    if bytes.is_empty() {
        return Err("The clipboard image was empty.".into());
    }
    prepare(dir)?;
    let path = free_path(dir, ext, now);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|err| format!("Could not save the image: {err}"))?;
    prune(dir, &path, now);
    Ok(path.to_string_lossy().into_owned())
}

/// Removes saved images older than [`KEEP`]. Best effort.
fn prune(dir: &Path, keep: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > KEEP));
        if old && path != keep && entry.file_name().to_string_lossy().starts_with("image-") {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Runs a command and returns its standard output, failing on a non-zero
/// exit or after [`TIMEOUT`].
fn output(command: &mut Command) -> Result<Vec<u8>, String> {
    let name = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("Could not run {name}: {err}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let bytes = rx.recv_timeout(TIMEOUT);
    if bytes.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    let bytes = bytes.map_err(|_| format!("{name} did not answer in time."))?;
    if !status.success() {
        return Err(format!("{name} failed."));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::write_executable;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

    /// A fake clipboard tool that answers from files in its directory.
    fn fake(dir: &Path, body: &str) -> PathBuf {
        let program = dir.join("tool");
        write_executable(&program, &format!("#!/bin/sh\n{body}\n"));
        program
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn wayland_saves_the_best_image_type_privately() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("png"), PNG).unwrap();
        let program = fake(
            tmp.path(),
            &format!(
                r#"case "$*" in
  --list-types) printf 'image/bmp\nimage/png\ntext/plain\n' ;;
  "--no-newline --type image/png") cat '{}/png' ;;
  *) exit 3 ;;
esac"#,
                tmp.path().display()
            ),
        );
        let dir = tmp.path().join("cache/herdr-inbox/images");
        let clip = read(&Tool::Wayland(program), &dir, at(1_700_000_000)).unwrap();
        let expected = dir.join("image-1700000000000.png");
        assert_eq!(clip, Clip::Image(expected.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read(&expected).unwrap(), PNG);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&expected).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(crate::images::is_saved_image(&expected.to_string_lossy()));
    }

    #[test]
    fn wayland_text_comes_back_as_text() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake(
            tmp.path(),
            r#"case "$*" in
  --list-types) printf 'text/plain;charset=utf-8\nUTF8_STRING\n' ;;
  "--no-newline --type text") printf 'hello\nworld' ;;
  *) exit 3 ;;
esac"#,
        );
        let dir = tmp.path().join("images");
        assert_eq!(read(&Tool::Wayland(program), &dir, at(1)).unwrap(), Clip::Text("hello\nworld".into()));
        assert!(!dir.exists(), "nothing is saved for text");
    }

    #[test]
    fn wayland_nothing_copied_or_unusable_types_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let nothing = fake(tmp.path(), "echo 'Nothing is copied' >&2; exit 1");
        assert_eq!(read(&Tool::Wayland(nothing), tmp.path(), at(1)).unwrap(), Clip::Empty);
        let bmp = fake(tmp.path(), "printf 'image/bmp\\n'");
        assert_eq!(read(&Tool::Wayland(bmp), tmp.path(), at(1)).unwrap(), Clip::Empty);
    }

    #[test]
    fn an_empty_or_failed_image_read_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = fake(tmp.path(), r#"[ "$1" = --list-types ] && printf 'image/png\n'; exit 0"#);
        assert_eq!(read(&Tool::Wayland(empty), tmp.path(), at(1)), Err("The clipboard image was empty.".into()));
        let failing = fake(tmp.path(), r#"[ "$1" = --list-types ] && printf 'image/png\n' && exit 0; exit 1"#);
        let error = read(&Tool::Wayland(failing), tmp.path(), at(1)).unwrap_err();
        assert!(error.ends_with("failed."), "{error}");
    }

    #[test]
    fn x11_reads_targets_then_the_image() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("jpg"), b"jpeg").unwrap();
        let program = fake(
            tmp.path(),
            &format!(
                r#"case "$*" in
  "-selection clipboard -t TARGETS -o") printf 'TARGETS\nimage/jpeg\n' ;;
  "-selection clipboard -t image/jpeg -o") cat '{}/jpg' ;;
  *) exit 3 ;;
esac"#,
                tmp.path().display()
            ),
        );
        let dir = tmp.path().join("images");
        let clip = read(&Tool::X11(program), &dir, at(2)).unwrap();
        assert_eq!(clip, Clip::Image(dir.join("image-2000.jpg").to_string_lossy().into_owned()));
    }

    #[test]
    fn x11_without_an_image_reads_text() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake(
            tmp.path(),
            r#"case "$*" in
  "-selection clipboard -t TARGETS -o") printf 'TARGETS\nUTF8_STRING\n' ;;
  "-selection clipboard -o") printf 'copied' ;;
  *) exit 3 ;;
esac"#,
        );
        assert_eq!(read(&Tool::X11(program), tmp.path(), at(1)).unwrap(), Clip::Text("copied".into()));
    }

    #[test]
    fn a_hanging_tool_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let program = fake(tmp.path(), "exec sleep 30");
        let started = std::time::Instant::now();
        assert_eq!(read(&Tool::Wayland(program), tmp.path(), at(1)).unwrap(), Clip::Empty);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn saving_never_overwrites_and_prunes_old_images_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let now = at(100 * 24 * 3600);
        let first = save(dir, "png", PNG, now).unwrap();
        let second = save(dir, "png", PNG, now).unwrap();
        assert_ne!(first, second);
        assert!(second.ends_with("-2.png"), "{second}");

        let old = dir.join("image-1.png");
        let notes = dir.join("notes.txt");
        std::fs::write(&old, PNG).unwrap();
        std::fs::write(&notes, "keep").unwrap();
        let ancient = std::fs::FileTimes::new().set_modified(at(1));
        for path in [&old, &notes] {
            std::fs::File::options().write(true).open(path).unwrap().set_times(ancient).unwrap();
        }
        // Real files are written now; pretend "now" is far in their future.
        let later = SystemTime::now() + Duration::from_secs(3600);
        save(dir, "png", PNG, later).unwrap();
        assert!(!old.exists(), "a month-old image is removed");
        assert!(notes.exists(), "other files are left alone");
        assert!(Path::new(&first).exists(), "recent images stay");
    }

    #[test]
    fn the_image_dir_follows_xdg_cache_home() {
        // Read-only check of the shape: the marker the app relies on.
        let dir = image_dir().expect("HOME is set in tests");
        assert!(format!("{}/", dir.display()).ends_with(crate::images::DIR_MARKER));
    }

    #[test]
    fn image_types_are_matched_whole() {
        assert_eq!(pick("image/png\n"), Some(("image/png", "png")));
        assert_eq!(pick("image/pngx\nimage/webp"), Some(("image/webp", "webp")));
        assert_eq!(pick("text/plain"), None);
    }
}
