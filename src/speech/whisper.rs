//! Local transcription: builds whisper.cpp in the user's data directory and
//! downloads a model, for dictation that never leaves the machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const REPO: &str = "https://github.com/ggml-org/whisper.cpp";
pub const MODEL: &str = "small";

pub fn model_url(model: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin")
}

/// `HERDR_INBOX_WHISPER_DIR`, else `~/.local/share/herdr-inbox/whisper`.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_INBOX_WHISPER_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    base.join(".local/share/herdr-inbox/whisper")
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The command template for a built whisper.cpp and model, if both exist.
pub fn command(home: &Path, model: &str) -> Option<String> {
    let binary = home.join("build/bin/whisper-cli");
    let weights = home.join("models").join(format!("ggml-{model}.bin"));
    (binary.is_file() && weights.is_file()).then(|| {
        format!(
            "{} -m {} -l auto -nt -np -f {{file}}",
            quote(&binary.display().to_string()),
            quote(&weights.display().to_string())
        )
    })
}

/// The programs the build needs, resolved by the caller.
pub struct Tools {
    pub git: Option<PathBuf>,
    pub cmake: Option<PathBuf>,
    pub compiler: bool,
    pub curl: Option<PathBuf>,
}

impl Tools {
    pub fn system() -> Self {
        let which = super::recorder::which;
        Self {
            git: which("git"),
            cmake: which("cmake"),
            compiler: ["c++", "g++", "clang++"].iter().any(|c| which(c).is_some()),
            curl: which("curl"),
        }
    }
}

fn step(program: &Path, args: &[&str], cwd: &Path, label: &str, progress: &dyn Fn(&str)) -> Result<(), String> {
    progress(label);
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("{label} failed: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text =
        String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr }).to_string();
    let count = text.chars().count();
    let tail: String = text.chars().skip(count.saturating_sub(400)).collect();
    Err(format!("{label} failed: {}", tail.trim()))
}

/// Builds whisper.cpp and downloads `model` under `home`, reporting each step.
/// Steps already done are skipped. Returns the command template.
pub fn install(tools: &Tools, home: &Path, model: &str, url: &str, progress: &dyn Fn(&str)) -> Result<String, String> {
    let git = tools.git.as_deref().ok_or("Local whisper needs git installed.")?;
    let cmake = tools.cmake.as_deref().ok_or("Local whisper needs cmake installed.")?;
    if !tools.compiler {
        return Err("Local whisper needs a C++ compiler (gcc or clang).".into());
    }
    let curl = tools.curl.as_deref().ok_or("Local whisper needs curl to download its model.")?;
    let parent = home.parent().ok_or("invalid whisper directory")?;
    std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    if !home.join("CMakeLists.txt").is_file() {
        let target = home.display().to_string();
        step(git, &["clone", "--depth", "1", REPO, &target], parent, "Cloning whisper.cpp", progress)?;
    }
    if !home.join("build/bin/whisper-cli").is_file() {
        let configure = ["-B", "build", "-DCMAKE_BUILD_TYPE=Release", "-DWHISPER_BUILD_TESTS=OFF", "-DGGML_NATIVE=ON"];
        step(cmake, &configure, home, "Configuring the build", progress)?;
        let jobs = std::thread::available_parallelism().map(|n| n.get().max(2)).unwrap_or(2).to_string();
        let build = ["--build", "build", "--config", "Release", "-j", &jobs, "--target", "whisper-cli"];
        step(cmake, &build, home, "Compiling whisper.cpp (a minute or two)", progress)?;
    }
    let weights = home.join("models").join(format!("ggml-{model}.bin"));
    if !weights.is_file() {
        download(curl, url, &weights, model, progress)?;
    }
    let command = command(home, model).ok_or("whisper.cpp did not produce a usable build")?;
    progress("Local whisper is ready");
    Ok(command)
}

/// Downloads to a `.part` file, reporting progress from its growing size,
/// then moves it into place so a broken download is never mistaken for a model.
fn download(curl: &Path, url: &str, weights: &Path, model: &str, progress: &dyn Fn(&str)) -> Result<(), String> {
    let folder = weights.parent().ok_or("invalid model path")?;
    std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
    let partial = weights.with_extension("part");
    let total = content_length(curl, url);
    let mut child = Command::new(curl)
        .args(["-sSL", "--fail", "-A", super::backends::USER_AGENT, "-o", &partial.display().to_string(), url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Model download failed: {err}"))?;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        let done = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        progress(&match total {
            Some(total) if total > 0 => format!("Downloading the {model} model… {}%", done * 100 / total),
            _ => format!("Downloading the {model} model… {} MB", done >> 20),
        });
        std::thread::sleep(Duration::from_millis(400));
    };
    if !status.success() {
        let mut detail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut detail);
        }
        let _ = std::fs::remove_file(&partial);
        return Err(format!("Model download failed: {}", detail.trim()));
    }
    std::fs::rename(&partial, weights).map_err(|err| format!("Model download failed: {err}"))
}

/// The size announced by the server, following redirects.
fn content_length(curl: &Path, url: &str) -> Option<u64> {
    let output = Command::new(curl).args(["-sIL", url]).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<u64>().ok())?
        })
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::write_executable;
    use std::sync::Mutex;

    fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        write_executable(
            &path,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}\n", dir.join("calls").display()),
        );
        path
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    /// Fake git, cmake and curl that do what the real ones would, on disk.
    fn tools(dir: &Path) -> Tools {
        Tools {
            git: Some(fake(dir, "git", "mkdir -p \"$5\" && touch \"$5/CMakeLists.txt\"")),
            cmake: Some(fake(dir, "cmake", "mkdir -p build/bin && touch build/bin/whisper-cli")),
            compiler: true,
            curl: Some(fake(
                dir,
                "curl",
                "case \"$1\" in -sIL) printf 'HTTP/1.1 302\\r\\nContent-Length: 0\\r\\n\\r\\nHTTP/1.1 200\\r\\nContent-Length: 10\\r\\n' ;; *) out=''; while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=\"$2\"; shift; done; printf '0123456789' > \"$out\" ;; esac",
            )),
        }
    }

    #[test]
    fn a_fresh_install_clones_builds_downloads_and_returns_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("data/whisper");
        let steps = Mutex::new(Vec::new());
        let command = install(&tools(dir.path()), &home, "small", "https://example/model.bin", &|s| {
            steps.lock().unwrap().push(s.to_string())
        })
        .unwrap();
        assert!(command.ends_with(" -l auto -nt -np -f {file}"), "{command}");
        assert!(command.contains("ggml-small.bin"));
        let steps = steps.into_inner().unwrap();
        assert_eq!(steps.first().map(String::as_str), Some("Cloning whisper.cpp"));
        assert!(steps.iter().any(|s| s == "Compiling whisper.cpp (a minute or two)"));
        assert_eq!(steps.last().map(String::as_str), Some("Local whisper is ready"));
        assert!(!home.join("models/ggml-small.part").exists(), "the partial file was moved into place");
        let calls = calls(dir.path());
        assert!(calls.iter().any(|c| c.starts_with("clone --depth 1 https://github.com/ggml-org/whisper.cpp")));
        assert!(calls.iter().any(|c| c.contains("--target whisper-cli")));
    }

    #[test]
    fn steps_already_done_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("whisper");
        std::fs::create_dir_all(home.join("build/bin")).unwrap();
        std::fs::create_dir_all(home.join("models")).unwrap();
        std::fs::write(home.join("CMakeLists.txt"), "").unwrap();
        std::fs::write(home.join("build/bin/whisper-cli"), "").unwrap();
        std::fs::write(home.join("models/ggml-small.bin"), "m").unwrap();
        install(&tools(dir.path()), &home, "small", "u", &|_| {}).unwrap();
        assert!(calls(dir.path()).is_empty(), "nothing to do");
    }

    #[test]
    fn missing_tools_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let mut missing = tools(dir.path());
        missing.cmake = None;
        assert_eq!(
            install(&missing, dir.path(), "small", "u", &|_| {}).unwrap_err(),
            "Local whisper needs cmake installed."
        );
        let mut missing = tools(dir.path());
        missing.compiler = false;
        assert!(install(&missing, dir.path(), "small", "u", &|_| {}).unwrap_err().contains("C++ compiler"));
    }

    #[test]
    fn a_failed_build_reports_the_end_of_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut broken = tools(dir.path());
        broken.cmake = Some(fake(dir.path(), "cmake", "echo 'CMake Error: no compiler' >&2; exit 1"));
        let err = install(&broken, &dir.path().join("w"), "small", "u", &|_| {}).unwrap_err();
        assert_eq!(err, "Configuring the build failed: CMake Error: no compiler");
    }

    #[test]
    fn a_failed_download_leaves_no_partial_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut offline = tools(dir.path());
        offline.curl = Some(fake(
            dir.path(),
            "curl",
            "case \"$1\" in -sIL) ;; *) echo 'Could not resolve host' >&2; exit 6 ;; esac",
        ));
        let home = dir.path().join("w");
        let err = install(&offline, &home, "small", "u", &|_| {}).unwrap_err();
        assert_eq!(err, "Model download failed: Could not resolve host");
        assert!(!home.join("models/ggml-small.part").exists());
        assert!(!home.join("models/ggml-small.bin").exists());
    }

    #[test]
    fn the_command_quotes_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("my whisper");
        assert_eq!(command(&home, "small"), None);
        std::fs::create_dir_all(home.join("build/bin")).unwrap();
        std::fs::create_dir_all(home.join("models")).unwrap();
        std::fs::write(home.join("build/bin/whisper-cli"), "").unwrap();
        std::fs::write(home.join("models/ggml-small.bin"), "").unwrap();
        let command = command(&home, "small").unwrap();
        assert!(command.starts_with(&format!("'{}/build/bin/whisper-cli'", home.display())), "{command}");
        assert_eq!(model_url("base"), "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin");
    }
}
