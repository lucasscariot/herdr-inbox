use std::cell::RefCell;

use super::*;
use crate::testing::write_executable;

fn settings() -> Settings {
    Settings::default()
}

fn environment<'a>(installed: &'a dyn Fn(&str) -> bool, env: &'a dyn Fn(&str) -> Option<String>) -> Environment<'a> {
    Environment { installed, env, model_dirs: Vec::new() }
}

#[test]
fn saved_credentials_win_over_the_config_and_keys_merge() {
    let config = SpeechConfig {
        backend: Some("openai".into()),
        command: Some("whisper {file}".into()),
        language: Some("fr".into()),
        model: None,
        keys: BTreeMap::from([("openai".into(), "sk-config".into()), ("groq".into(), "gsk-config".into())]),
    };
    let saved = Credentials {
        backend: "groq".into(),
        keys: BTreeMap::from([("groq".into(), "gsk-saved".into())]),
        command: String::new(),
    };
    let merged = Settings::merge(&config, &saved);
    assert_eq!(merged.backend.as_deref(), Some("groq"));
    assert_eq!(merged.command.as_deref(), Some("whisper {file}"), "an empty saved command keeps the configured one");
    assert_eq!(merged.keys["groq"], "gsk-saved");
    assert_eq!(merged.keys["openai"], "sk-config");
    assert_eq!(merged.language.as_deref(), Some("fr"));
}

#[test]
fn the_legacy_credentials_file_reads_as_is() {
    let saved: Credentials = serde_json::from_str(r#"{"backend": "", "keys": {"groq": "gsk_x"}}"#).unwrap();
    assert_eq!(saved.keys["groq"], "gsk_x");
    assert_eq!(saved.command, "");
}

#[test]
fn keys_come_from_settings_then_the_environment() {
    let env = |name: &str| (name == "GOOGLE_API_KEY").then(|| " g-env ".to_string());
    let none = |_: &str| false;
    let environment = environment(&none, &env);
    let mut settings = settings();
    assert_eq!(key_for("gemini", &settings, &environment).as_deref(), Some("g-env"), "the second variable counts too");
    settings.keys.insert("gemini".into(), "g-saved".into());
    assert_eq!(key_for("gemini", &settings, &environment).as_deref(), Some("g-saved"));
    settings.keys.insert("groq".into(), "  ".into());
    assert_eq!(key_for("groq", &settings, &environment), None, "a blank key is no key");
    assert_eq!(key_for("nope", &settings, &environment), None);
}

#[test]
fn backends_are_tried_command_then_local_tools_then_services() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ggml-small.bin"), "m").unwrap();
    let installed = |name: &str| matches!(name, "voxtype" | "whisper-cli");
    let env = |name: &str| (name == "DEEPGRAM_API_KEY").then(|| "dg".to_string());
    let environment = Environment { installed: &installed, env: &env, model_dirs: vec![dir.path().into()] };
    let mut settings = settings();
    settings.command = Some("my-stt {file}".into());
    settings.keys.insert("groq".into(), "gsk".into());
    let ids: Vec<String> = available(&settings, &environment).iter().map(|b| backend_id(b).to_string()).collect();
    assert_eq!(ids, ["command", "voxtype", "whisper-cli", "groq", "deepgram"]);
    settings.backend = Some("deepgram".into());
    let forced = available(&settings, &environment);
    assert_eq!(forced, vec![Backend::Service { id: "deepgram", key: "dg".into() }]);
    settings.backend = Some("openai".into());
    assert!(available(&settings, &environment).is_empty(), "a forced backend without a key is not replaced");
}

#[test]
fn whisper_needs_a_model_to_count() {
    let installed = |name: &str| name == "whisper-cli";
    let env = |_: &str| None;
    assert!(available(&settings(), &environment(&installed, &env)).is_empty());
}

#[test]
fn a_configured_model_file_wins_over_found_ones() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ggml-base.bin"), "m").unwrap();
    std::fs::write(dir.path().join("ggml-tiny.bin"), "m").unwrap();
    std::fs::write(dir.path().join("readme.txt"), "x").unwrap();
    let mut settings = settings();
    assert_eq!(find_model(&settings, &[dir.path().into()]), Some(dir.path().join("ggml-base.bin")), "sorted, first");
    let mine = dir.path().join("mine.bin");
    std::fs::write(&mine, "m").unwrap();
    settings.model = Some(mine.display().to_string());
    assert_eq!(find_model(&settings, &[dir.path().into()]), Some(mine));
    assert_eq!(find_model(&Settings::default(), &[dir.path().join("missing")]), None);
}

#[test]
fn backends_describe_themselves() {
    assert_eq!(Backend::Command("whisper-cli -f {file}".into()).describe(), "local whisper.cpp");
    assert_eq!(Backend::Command("stt {file}".into()).describe(), "local command");
    assert_eq!(Backend::Tool { name: "voxtype", model: None }.describe(), "voxtype (local)");
    assert_eq!(Backend::Service { id: "groq", key: "k".into() }.describe(), "Groq Whisper");
}

#[test]
fn the_transcript_is_the_last_paragraph_without_escapes() {
    let output = "\x1b[32mloading model\x1b[0m\nprogress 100%\n\n  Fix the login loop.  \n\n";
    assert_eq!(last_paragraph(output), "Fix the login loop.");
    assert_eq!(last_paragraph(""), "");
}

#[test]
fn a_command_template_gets_the_quoted_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("it's here.wav");
    std::fs::write(&file, "x").unwrap();
    let text =
        transcribe_with(&Backend::Command("printf 'heard %s' {file}".into()), &file, &settings(), "curl").unwrap();
    assert_eq!(text, format!("heard {}", file.display()));
    let appended = transcribe_with(&Backend::Command("printf 'got %s'".into()), &file, &settings(), "curl").unwrap();
    assert_eq!(appended, format!("got {}", file.display()), "without {{file}} the path is appended");
    let failing =
        transcribe_with(&Backend::Command("echo broken >&2; exit 1 # {file}".into()), &file, &settings(), "curl");
    assert_eq!(failing.unwrap_err(), "broken");
}

/// A fake curl: saves its arguments and stdin config, prints `body` and
/// `status` the way `-w '\n%{http_code}'` would.
struct FakeCurl {
    dir: tempfile::TempDir,
    path: String,
}

impl FakeCurl {
    fn new(body: &str, status: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("curl");
        write_executable(
            &script,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{args}'\ncat > '{config}'\nprintf '%s\\n%s' '{body}' '{status}'\n",
                args = dir.path().join("args").display(),
                config = dir.path().join("config").display(),
                body = body.replace('\'', "'\\''"),
            ),
        );
        Self { path: script.display().to_string(), dir }
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("args")).unwrap().lines().map(str::to_string).collect()
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("config")).unwrap()
    }
}

fn recording() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rec.wav");
    std::fs::write(&path, vec![1u8; 2048]).unwrap();
    (dir, path)
}

#[test]
fn openai_style_services_post_multipart_with_the_key_on_stdin() {
    let curl = FakeCurl::new(r#"{"text": " Fix the login loop. "}"#, 200);
    let (_dir, path) = recording();
    let mut settings = settings();
    settings.language = Some("en".into());
    let text =
        transcribe_with(&Backend::Service { id: "groq", key: "gsk_SECRET".into() }, &path, &settings, &curl.path)
            .unwrap();
    assert_eq!(text, "Fix the login loop.");
    let args = curl.args();
    assert!(!args.iter().any(|a| a.contains("gsk_SECRET")), "the key never reaches the command line: {args:?}");
    assert!(args.windows(2).any(|w| w == ["-F", "model=whisper-large-v3-turbo"]));
    assert!(args.windows(2).any(|w| w == ["-F", "language=en"]));
    assert!(args.iter().any(|a| a == &format!("file=@{};type=audio/wav", path.display())));
    let config = curl.config();
    assert!(config.contains("header = \"Authorization: Bearer gsk_SECRET\""), "{config}");
    assert!(config.contains("url = \"https://api.groq.com/openai/v1/audio/transcriptions\""));
    assert!(config.contains("User-Agent: herdr-inbox/"), "Cloudflare rejects agentless requests");
}

#[test]
fn deepgram_sends_raw_audio_with_a_token() {
    let curl = FakeCurl::new(r#"{"results": {"channels": [{"alternatives": [{"transcript": "hello there"}]}]}}"#, 200);
    let (_dir, path) = recording();
    let text = transcribe_with(&Backend::Service { id: "deepgram", key: "dg".into() }, &path, &settings(), &curl.path)
        .unwrap();
    assert_eq!(text, "hello there");
    assert!(curl.config().contains("Authorization: Token dg"));
    assert!(curl.config().contains("listen?model=nova-3&smart_format=true"));
    assert!(curl.args().contains(&format!("@{}", path.display())));
}

#[test]
fn gemini_gets_its_key_in_the_url_on_stdin_and_the_audio_as_json() {
    let curl = FakeCurl::new(r#"{"candidates": [{"content": {"parts": [{"text": "Ship "}, {"text": "it"}]}}]}"#, 200);
    let (_dir, path) = recording();
    let text =
        transcribe_with(&Backend::Service { id: "gemini", key: "AIzaSECRET".into() }, &path, &settings(), &curl.path)
            .unwrap();
    assert_eq!(text, "Ship it");
    assert!(!curl.args().iter().any(|a| a.contains("AIzaSECRET")));
    assert!(curl.config().contains("gemini-2.5-flash:generateContent?key=AIzaSECRET"));
    assert!(!curl.config().contains("Authorization"), "Gemini takes no header");
}

#[test]
fn service_errors_carry_the_services_message() {
    let curl = FakeCurl::new(r#"{"error": {"message": "Invalid API Key"}}"#, 401);
    let (_dir, path) = recording();
    let err = transcribe_with(&Backend::Service { id: "openai", key: "sk".into() }, &path, &settings(), &curl.path)
        .unwrap_err();
    assert_eq!(err, "HTTP 401: Invalid API Key");
    let curl = FakeCurl::new("<html>gateway</html>", 502);
    let err = transcribe_with(&Backend::Service { id: "openai", key: "sk".into() }, &path, &settings(), &curl.path)
        .unwrap_err();
    assert_eq!(err, "HTTP 502: <html>gateway</html>");
    let curl = FakeCurl::new(r#"{"results": {}}"#, 200);
    let err = transcribe_with(&Backend::Service { id: "deepgram", key: "k".into() }, &path, &settings(), &curl.path)
        .unwrap_err();
    assert_eq!(err, "Deepgram returned no transcript");
}

#[test]
fn keys_are_verified_with_a_cheap_request() {
    let curl = FakeCurl::new("{}", 200);
    verify_key("openai", " sk-ok ", &curl.path).unwrap();
    assert!(curl.config().contains("url = \"https://api.openai.com/v1/models\""));
    assert!(curl.config().contains("Bearer sk-ok"), "trimmed");
    let curl = FakeCurl::new(r#"{"message": "bad key"}"#, 403);
    assert_eq!(
        verify_key("mistral", "k", &curl.path).unwrap_err(),
        "Mistral Voxtral rejected the key: HTTP 403: bad key"
    );
    assert_eq!(verify_key("groq", "has space", &curl.path).unwrap_err(), "That does not look like an API key.");
    assert!(verify_key("nope", "k", &curl.path).is_err());
}

#[test]
fn without_curl_the_service_says_what_to_install() {
    let (_dir, path) = recording();
    let err =
        transcribe_with(&Backend::Service { id: "groq", key: "k".into() }, &path, &settings(), "/nonexistent/curl")
            .unwrap_err();
    assert!(err.contains("Install curl"), "{err}");
}

#[test]
fn transcribe_explains_a_missing_backend_and_an_empty_result() {
    let (_dir, path) = recording();
    let none = |_: &str| false;
    let env = |_: &str| None;
    let err = transcribe(&path, &settings(), &environment(&none, &env), "curl").unwrap_err();
    assert!(err.starts_with("No transcription service is connected."), "{err}");
    let mut forced = settings();
    forced.backend = Some("groq".into());
    let err = transcribe(&path, &forced, &environment(&none, &env), "curl").unwrap_err();
    assert!(err.starts_with("Speech backend groq is not available."), "{err}");
    let mut silent = settings();
    silent.command = Some("printf ''".into());
    let err = transcribe(&path, &silent, &environment(&none, &env), "curl").unwrap_err();
    assert!(err.starts_with("Nothing was transcribed"), "{err}");
    let calls = RefCell::new(0);
    let _ = &calls;
}
