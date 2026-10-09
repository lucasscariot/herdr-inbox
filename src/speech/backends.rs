//! Turning a recording into text: a custom command, a local tool (voxtype,
//! whisper.cpp), or a hosted service with an API key.
//!
//! Hosted services are called with `curl`. Keys and URLs go to curl on its
//! standard input as a config file, never on the command line, where other
//! users could read them in the process list.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::recorder;

pub const USER_AGENT: &str = concat!("herdr-inbox/", env!("CARGO_PKG_VERSION"), " (+https://herdr.dev)");
pub const SETUP_HINT: &str = "Press F10 to connect a transcription service or set up local whisper.cpp.";
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(300);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// `[speech]` in config.toml.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SpeechConfig {
    pub backend: Option<String>,
    pub command: Option<String>,
    pub language: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Holding the space bar dictates; `false` keeps space for typing only.
    pub space_hold: Option<bool>,
}

/// What the dictation menu saved: `credentials.json`, in the legacy format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    #[serde(default)]
    pub command: String,
}

/// Config and saved credentials together; the saved ones win.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub backend: Option<String>,
    pub command: Option<String>,
    pub language: Option<String>,
    pub model: Option<String>,
    pub keys: BTreeMap<String, String>,
}

impl Settings {
    pub fn merge(config: &SpeechConfig, saved: &Credentials) -> Self {
        let mut keys = config.keys.clone();
        keys.extend(saved.keys.clone());
        let pick = |saved: &str, configured: &Option<String>| {
            Some(saved.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| configured.clone().filter(|c| !c.trim().is_empty()))
        };
        Self {
            backend: pick(&saved.backend, &config.backend),
            command: pick(&saved.command, &config.command),
            language: config.language.clone().filter(|l| !l.trim().is_empty()),
            model: config.model.clone().filter(|m| !m.trim().is_empty()),
            keys,
        }
    }
}

pub struct Service {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub env: &'static [&'static str],
    pub model: &'static str,
    pub keys_url: &'static str,
    /// A cheap authenticated GET that tells whether a key works.
    pub verify: &'static str,
}

pub const SERVICES: [Service; 5] = [
    Service {
        id: "groq",
        label: "Groq Whisper",
        detail: "free tier · fastest",
        env: &["GROQ_API_KEY"],
        model: "whisper-large-v3-turbo",
        keys_url: "https://console.groq.com/keys",
        verify: "https://api.groq.com/openai/v1/models",
    },
    Service {
        id: "gemini",
        label: "Google Gemini",
        detail: "free tier",
        env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        model: "gemini-2.5-flash",
        keys_url: "https://aistudio.google.com/apikey",
        verify: "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1&key={key}",
    },
    Service {
        id: "openai",
        label: "OpenAI",
        detail: "pay per minute",
        env: &["OPENAI_API_KEY"],
        model: "gpt-4o-mini-transcribe",
        keys_url: "https://platform.openai.com/api-keys",
        verify: "https://api.openai.com/v1/models",
    },
    Service {
        id: "mistral",
        label: "Mistral Voxtral",
        detail: "pay per minute",
        env: &["MISTRAL_API_KEY"],
        model: "voxtral-mini-latest",
        keys_url: "https://console.mistral.ai/api-keys",
        verify: "https://api.mistral.ai/v1/models",
    },
    Service {
        id: "deepgram",
        label: "Deepgram Nova",
        detail: "pay per minute",
        env: &["DEEPGRAM_API_KEY"],
        model: "nova-3",
        keys_url: "https://console.deepgram.com/",
        verify: "https://api.deepgram.com/v1/projects",
    },
];

pub fn service(id: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.id == id)
}

/// Where whisper.cpp models are commonly kept.
pub fn model_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/share/herdr-inbox/whisper/models"),
        home.join(".local/share/whisper.cpp/models"),
        home.join(".cache/whisper.cpp"),
        home.join(".local/share/voxtype/models"),
        PathBuf::from("/usr/share/whisper.cpp/models"),
        PathBuf::from("/usr/local/share/whisper.cpp/models"),
        PathBuf::from("/opt/homebrew/share/whisper-cpp/models"),
    ]
}

/// A whisper.cpp model: the configured `.bin`, else the first `ggml-*.bin`.
pub fn find_model(settings: &Settings, dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(model) = settings.model.as_deref().filter(|m| m.ends_with(".bin")) {
        let path = expand_home(model);
        if path.is_file() {
            return Some(path);
        }
    }
    dirs.iter().find_map(|dir| {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ggml-") && n.ends_with(".bin"))
            })
            .collect();
        found.sort();
        found.into_iter().next()
    })
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// A command template; `{file}` is the recording.
    Command(String),
    /// voxtype, whisper-cli or whisper-cpp found on PATH.
    Tool {
        name: &'static str,
        model: Option<PathBuf>,
    },
    Service {
        id: &'static str,
        key: String,
    },
}

impl Backend {
    pub fn describe(&self) -> String {
        match self {
            Backend::Command(command) if command.contains("whisper") => "local whisper.cpp".into(),
            Backend::Command(_) => "local command".into(),
            Backend::Tool { name: "voxtype", .. } => "voxtype (local)".into(),
            Backend::Tool { .. } => "whisper.cpp (local)".into(),
            Backend::Service { id, .. } => service(id).map(|s| s.label.to_string()).unwrap_or_default(),
        }
    }
}

/// What the environment offers: installed tools, environment keys, models.
pub struct Environment<'a> {
    pub installed: &'a dyn Fn(&str) -> bool,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub model_dirs: Vec<PathBuf>,
}

impl Environment<'_> {
    pub fn system() -> Environment<'static> {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        Environment {
            installed: &recorder::installed,
            env: &|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()),
            model_dirs: model_dirs(&home),
        }
    }
}

pub fn key_for(id: &str, settings: &Settings, environment: &Environment) -> Option<String> {
    if let Some(key) = settings.keys.get(id).map(|k| k.trim()).filter(|k| !k.is_empty()) {
        return Some(key.to_string());
    }
    service(id)?.env.iter().find_map(|name| (environment.env)(name)).map(|k| k.trim().to_string())
}

/// Every usable backend, in the order they would be tried. A forced backend
/// is the only one considered.
pub fn available(settings: &Settings, environment: &Environment) -> Vec<Backend> {
    let mut backends = Vec::new();
    if let Some(command) = &settings.command {
        backends.push(Backend::Command(command.clone()));
    }
    let model = find_model(settings, &environment.model_dirs);
    if (environment.installed)("voxtype") {
        backends.push(Backend::Tool { name: "voxtype", model: None });
    }
    for name in ["whisper-cli", "whisper-cpp"] {
        if (environment.installed)(name) && model.is_some() {
            backends.push(Backend::Tool { name, model: model.clone() });
        }
    }
    for service in &SERVICES {
        if let Some(key) = key_for(service.id, settings, environment) {
            backends.push(Backend::Service { id: service.id, key });
        }
    }
    match settings.backend.as_deref() {
        None => backends,
        Some(forced) => backends.into_iter().filter(|b| backend_id(b) == forced).collect(),
    }
}

fn backend_id(backend: &Backend) -> &str {
    match backend {
        Backend::Command(_) => "command",
        Backend::Tool { name, .. } => name,
        Backend::Service { id, .. } => id,
    }
}

/// Single-quotes a path for `sh -c`.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The transcript: the last non-empty paragraph, without ANSI escapes.
/// Transcribers print progress first.
pub fn last_paragraph(output: &str) -> String {
    let clean: String = strip_ansi(output);
    clean
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .last()
        .unwrap_or("")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Transcribes with the first available backend.
pub fn transcribe(path: &Path, settings: &Settings, environment: &Environment, curl: &str) -> Result<String, String> {
    let backends = available(settings, environment);
    let Some(backend) = backends.first() else {
        return Err(match &settings.backend {
            Some(forced) => format!("Speech backend {forced} is not available. {SETUP_HINT}"),
            None => format!("No transcription service is connected. {SETUP_HINT}"),
        });
    };
    let text = transcribe_with(backend, path, settings, curl).map_err(|err| format!("Transcription failed: {err}"))?;
    if text.trim().is_empty() {
        return Err("Nothing was transcribed. Try speaking closer to the microphone.".into());
    }
    Ok(text.trim().to_string())
}

pub fn transcribe_with(backend: &Backend, path: &Path, settings: &Settings, curl: &str) -> Result<String, String> {
    let file = path.display().to_string();
    match backend {
        Backend::Command(template) => {
            let script = if template.contains("{file}") {
                template.replace("{file}", &quote(&file))
            } else {
                format!("{template} {}", quote(&file))
            };
            let output = run(Command::new("sh").arg("-c").arg(script), None, TRANSCRIBE_TIMEOUT)?;
            Ok(output.trim().to_string())
        }
        Backend::Tool { name: "voxtype", .. } => {
            let output = run(Command::new("voxtype").args(["-q", "transcribe", &file]), None, TRANSCRIBE_TIMEOUT)?;
            Ok(last_paragraph(&output))
        }
        Backend::Tool { name, model } => {
            let model = model.as_ref().ok_or("no whisper.cpp model found")?;
            let output = run(
                Command::new(name).args(["-m", &model.display().to_string(), "-l", "auto", "-nt", "-np", "-f", &file]),
                None,
                TRANSCRIBE_TIMEOUT,
            )?;
            Ok(output.trim().to_string())
        }
        Backend::Service { id, key } => service_transcribe(id, key, path, settings, curl),
    }
}

/// A curl config file: one directive per line, values quoted.
fn curl_config(lines: &[(&str, String)]) -> String {
    lines
        .iter()
        .map(|(name, value)| format!("{name} = \"{}\"\n", value.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect()
}

fn auth_header(id: &str, key: &str) -> Option<String> {
    match id {
        "deepgram" => Some(format!("Authorization: Token {key}")),
        "gemini" => None,
        _ => Some(format!("Authorization: Bearer {key}")),
    }
}

fn service_transcribe(id: &str, key: &str, path: &Path, settings: &Settings, curl: &str) -> Result<String, String> {
    let service = service(id).ok_or_else(|| format!("unknown service {id}"))?;
    let model = settings.model.clone().filter(|m| !m.ends_with(".bin")).unwrap_or_else(|| service.model.to_string());
    let file = path.display().to_string();
    let mut config: Vec<(&str, String)> =
        vec![("header", format!("User-Agent: {USER_AGENT}")), ("header", "Accept: application/json".into())];
    config.extend(auth_header(id, key).map(|h| ("header", h)));
    let mut args: Vec<String> = Vec::new();
    let mut _payload: Option<tempfile::NamedTempFile> = None;
    match id {
        "openai" | "groq" | "mistral" => {
            let url = match id {
                "openai" => "https://api.openai.com/v1/audio/transcriptions",
                "groq" => "https://api.groq.com/openai/v1/audio/transcriptions",
                _ => "https://api.mistral.ai/v1/audio/transcriptions",
            };
            config.push(("url", url.into()));
            args.extend(["-F".into(), format!("model={model}"), "-F".into(), "response_format=json".into()]);
            if let Some(language) = &settings.language {
                args.extend(["-F".into(), format!("language={language}")]);
            }
            args.extend(["-F".into(), format!("file=@{file};type=audio/wav")]);
        }
        "deepgram" => {
            let mut url = format!("https://api.deepgram.com/v1/listen?model={model}&smart_format=true");
            if let Some(language) = &settings.language {
                url.push_str(&format!("&language={language}"));
            }
            config.push(("url", url));
            config.push(("header", "Content-Type: audio/wav".into()));
            args.extend(["--data-binary".into(), format!("@{file}")]);
        }
        "gemini" => {
            use base64::Engine;
            let audio = std::fs::read(path).map_err(|err| err.to_string())?;
            let prompt = format!(
                "Transcribe this audio verbatim{}. Reply with the transcript only, no quotes or commentary.",
                settings.language.as_deref().map(|l| format!(" in {l}")).unwrap_or_default()
            );
            let body = serde_json::json!({"contents": [{"parts": [
                {"text": prompt},
                {"inline_data": {"mime_type": "audio/wav", "data": base64::engine::general_purpose::STANDARD.encode(audio)}},
            ]}]});
            let mut payload = tempfile::NamedTempFile::new().map_err(|err| err.to_string())?;
            payload.write_all(body.to_string().as_bytes()).map_err(|err| err.to_string())?;
            config.push((
                "url",
                format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={key}"),
            ));
            config.push(("header", "Content-Type: application/json".into()));
            args.extend(["--data-binary".into(), format!("@{}", payload.path().display())]);
            _payload = Some(payload);
        }
        other => return Err(format!("unknown service {other}")),
    }
    let response = http(curl, &config, &args, TRANSCRIBE_TIMEOUT)?;
    let text = match id {
        "deepgram" => response
            .pointer("/results/channels/0/alternatives/0/transcript")
            .and_then(Value::as_str)
            .ok_or("Deepgram returned no transcript")?
            .to_string(),
        "gemini" => {
            let parts = response
                .pointer("/candidates/0/content/parts")
                .and_then(Value::as_array)
                .ok_or("Gemini returned no transcript")?;
            parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect()
        }
        _ => response.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
    };
    Ok(text.trim().to_string())
}

/// Checks a pasted key with a cheap authenticated request.
pub fn verify_key(id: &str, key: &str, curl: &str) -> Result<(), String> {
    let service = service(id).ok_or_else(|| format!("Unknown service {id}"))?;
    let key = key.trim();
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return Err("That does not look like an API key.".into());
    }
    let mut config =
        vec![("header", format!("User-Agent: {USER_AGENT}")), ("url", service.verify.replace("{key}", key))];
    config.extend(auth_header(id, key).map(|h| ("header", h)));
    http(curl, &config, &[], VERIFY_TIMEOUT)
        .map(|_| ())
        .map_err(|err| format!("{} rejected the key: {err}", service.label))
}

/// Runs curl with a config on stdin and returns the JSON body. A status
/// outside 2xx is an error with the service's own message.
fn http(curl: &str, config: &[(&str, String)], args: &[String], timeout: Duration) -> Result<Value, String> {
    if !recorder::installed(curl) {
        return Err("curl is needed to reach transcription services. Install curl.".into());
    }
    let mut command = Command::new(curl);
    command.args(["-sS", "-K", "-", "-w", "\n%{http_code}", "--max-time", &timeout.as_secs().to_string()]).args(args);
    let output = run(&mut command, Some(&curl_config(config)), timeout + Duration::from_secs(5))?;
    let (body, status) = output.trim_end().rsplit_once('\n').unwrap_or(("", output.trim()));
    let status: u16 = status.trim().parse().map_err(|_| format!("no answer: {}", output.trim()))?;
    let parsed: Value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).unwrap_or(Value::String(body.into()))
    };
    if (200..300).contains(&status) {
        return Ok(parsed);
    }
    let message = ["/error/message", "/message", "/err_msg", "/error"]
        .iter()
        .find_map(|p| parsed.pointer(p).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(300).collect());
    Err(format!("HTTP {status}: {}", message.trim()))
}

/// Runs a command to completion, feeding `stdin`, and returns stdout. A
/// failure carries the end of stderr.
fn run(command: &mut Command, stdin: Option<&str>, timeout: Duration) -> Result<String, String> {
    use std::io::Read;
    let mut child = command
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{:?} could not start: {err}", command.get_program()))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(input.as_bytes());
    }
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut out, mut err) = (String::new(), String::new());
        let _ = stdout.read_to_string(&mut out);
        let _ = stderr.read_to_string(&mut err);
        let _ = tx.send((out, err));
    });
    let Ok((out, err)) = rx.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("timed out".into());
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(out)
    } else {
        let detail = strip_ansi(if err.trim().is_empty() { &out } else { &err });
        let detail = detail.trim();
        let count = detail.chars().count();
        Err(detail.chars().skip(count.saturating_sub(300)).collect::<String>().trim().to_string())
    }
}

#[cfg(test)]
mod tests;
