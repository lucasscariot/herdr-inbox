//! Recording from the microphone with whatever recorder is installed. Every
//! recorder writes a 16 kHz mono WAV to disk as it captures, which the meter
//! tails.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A finished recording smaller than this holds no speech, just a header.
const MIN_BYTES: u64 = 1024;
const STOP_GRACE: Duration = Duration::from_secs(3);

/// The first installed recorder wins.
pub fn recorder_argv(path: &Path, installed: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let path = path.display().to_string();
    let mac = cfg!(target_os = "macos");
    let candidates: [(&str, Vec<String>); 5] = [
        (
            "pw-record",
            vec!["pw-record", "--rate", "16000", "--channels", "1", "--format", "s16"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        (
            "arecord",
            vec!["arecord", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1"].into_iter().map(String::from).collect(),
        ),
        (
            "parecord",
            vec!["parecord", "--rate=16000", "--channels=1", "--format=s16le", "--file-format=wav"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        ("rec", vec!["rec", "-q", "-r", "16000", "-c", "1", "-b", "16"].into_iter().map(String::from).collect()),
        (
            "ffmpeg",
            vec![
                "ffmpeg",
                "-loglevel",
                "error",
                "-y",
                "-f",
                if mac { "avfoundation" } else { "pulse" },
                "-i",
                if mac { ":0" } else { "default" },
                "-ac",
                "1",
                "-ar",
                "16000",
                "-flush_packets",
                "1",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        ),
    ];
    candidates.into_iter().find(|(name, _)| installed(name)).map(|(_, mut argv)| {
        argv.push(path.clone());
        argv
    })
}

/// `name` resolved on PATH, like `which`.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

pub fn installed(name: &str) -> bool {
    which(name).is_some()
}

pub struct Recording {
    child: Child,
    path: PathBuf,
    started: Instant,
    stderr: Arc<Mutex<String>>,
}

impl Recording {
    /// A fresh WAV path in the temporary directory.
    pub fn new_path() -> PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        );
        std::env::temp_dir().join(format!("herdr-inbox-{unique}.wav"))
    }

    pub fn start(argv: &[String], path: PathBuf) -> Result<Self, String> {
        let (program, args) = argv.split_first().ok_or("no recorder")?;
        let mut command = Command::new(program);
        command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        {
            use std::os::unix::process::CommandExt;
            // Its own process group: a Ctrl+C meant for the inbox never cuts it.
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|err| format!("Could not start {program}: {err}"))?;
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut pipe) = child.stderr.take() {
            let stderr = Arc::clone(&stderr);
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                *stderr.lock().unwrap_or_else(|e| e.into_inner()) = text;
            });
        }
        Ok(Self { child, path, started: Instant::now(), stderr })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Stops the recorder with SIGINT so it finalizes the WAV header, then
    /// returns the file. An empty recording is an error that says why.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        if let Ok(None) = self.child.try_wait() {
            let pid = self.child.id() as libc::pid_t;
            // SAFETY: `kill` with a child's pid and a standard signal has no
            // memory effects; the worst case is ESRCH if it already exited.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
            let deadline = Instant::now() + STOP_GRACE;
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        let size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size < MIN_BYTES {
            std::thread::sleep(Duration::from_millis(20));
            let detail = self.stderr.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
            let _ = std::fs::remove_file(&self.path);
            let detail =
                if detail.is_empty() { "Check the microphone and its input level.".to_string() } else { detail };
            return Err(format!("The recording is empty. {}", tail(&detail, 300)));
        }
        Ok(self.path)
    }

    /// Stops recording and deletes the file.
    pub fn cancel(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The last `chars` characters of `text`.
fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(chars)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::write_executable;

    #[test]
    fn the_first_installed_recorder_is_used() {
        let path = Path::new("/tmp/x.wav");
        let argv = recorder_argv(path, &|name| name == "arecord" || name == "ffmpeg").unwrap();
        assert_eq!(argv, ["arecord", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "/tmp/x.wav"]);
        let argv = recorder_argv(path, &|name| name == "ffmpeg").unwrap();
        assert_eq!(argv.first().map(String::as_str), Some("ffmpeg"));
        assert_eq!(argv.last().map(String::as_str), Some("/tmp/x.wav"));
        assert!(recorder_argv(path, &|_| false).is_none());
    }

    #[test]
    fn which_finds_executables_only() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-command-xyz").is_none());
        assert!(which("/bin/sh").is_some());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plain"), "x").unwrap();
        assert!(which(&dir.path().join("plain").display().to_string()).is_some(), "a path is taken as is");
    }

    /// A recorder that writes a header and samples, and finalizes on SIGINT.
    fn fake_recorder(dir: &Path, samples: usize) -> String {
        let script = dir.join("recorder");
        write_executable(
            &script,
            &format!(
                "#!/bin/sh\nout=\"$1\"\ntrap 'printf done >> \"$out\"; exit 0' INT\nprintf 'RIFF' > \"$out\"\nhead -c {samples} /dev/zero >> \"$out\"\nwhile true; do sleep 0.05; done\n"
            ),
        );
        script.display().to_string()
    }

    #[test]
    fn stopping_sends_sigint_and_returns_the_finished_file() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = fake_recorder(dir.path(), 4096);
        let path = dir.path().join("rec.wav");
        let recording = Recording::start(&[recorder, path.display().to_string()], path.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(recording.elapsed() >= Duration::from_millis(200));
        let finished = recording.stop().unwrap();
        let bytes = std::fs::read(&finished).unwrap();
        assert!(bytes.ends_with(b"done"), "the recorder got SIGINT and finalized the file");
    }

    #[test]
    fn an_empty_recording_is_an_error_with_the_recorders_words() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("recorder");
        write_executable(&script, "#!/bin/sh\necho 'audio open error: Device or resource busy' >&2\nexit 1\n");
        let path = dir.path().join("rec.wav");
        let recording =
            Recording::start(&[script.display().to_string(), path.display().to_string()], path.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let err = recording.stop().unwrap_err();
        assert_eq!(err, "The recording is empty. audio open error: Device or resource busy");
        assert!(!path.exists());
    }

    #[test]
    fn cancelling_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = fake_recorder(dir.path(), 4096);
        let path = dir.path().join("rec.wav");
        let recording = Recording::start(&[recorder, path.display().to_string()], path.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        recording.cancel();
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_recorder_says_so() {
        let err = Recording::start(&["/nonexistent/rec".into()], "/tmp/x.wav".into()).err().unwrap();
        assert!(err.starts_with("Could not start /nonexistent/rec"), "{err}");
        assert!(Recording::start(&[], "/tmp/x.wav".into()).is_err());
    }

    #[test]
    fn new_paths_are_unique_temporary_wavs() {
        let a = Recording::new_path();
        let b = Recording::new_path();
        assert_ne!(a, b);
        assert!(a.extension().is_some_and(|e| e == "wav"));
    }
}
