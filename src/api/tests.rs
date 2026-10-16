use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;

use super::*;

const ORIGIN: &str = "https://dash.example.com";
const LOGIN: &str = "me@example.com";

fn config() -> ApiConfig {
    ApiConfig { enabled: true, port: 0, allowed_origin: ORIGIN.into(), allowed_login: LOGIN.into() }
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        headers: headers.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.to_string())).collect(),
        body: body.as_bytes().to_vec(),
    }
}

fn post(headers: &[(&str, &str)], body: &str) -> Route {
    route(&request("POST", "/prompts", headers, body), &config())
}

const BODY: &str = r#"{"repository": " me/app ", "prompt": "Fix the build"}"#;

fn status(route: Route) -> u16 {
    match route {
        Route::Respond(response) => response.status,
        Route::Prompt { .. } => 0,
    }
}

#[test]
fn a_prompt_from_the_allowed_page_and_login_goes_to_the_app() {
    let headers =
        [("Origin", ORIGIN), ("Tailscale-User-Login", "Me@Example.com"), ("Content-Type", "application/json")];
    assert_eq!(post(&headers, BODY), Route::Prompt { repository: "me/app".into(), prompt: "Fix the build".into() });
}

#[test]
fn anything_else_is_refused_before_the_app_sees_it() {
    let json = ("Content-Type", "application/json");
    let login = ("Tailscale-User-Login", LOGIN);
    assert_eq!(status(route(&request("POST", "/elsewhere", &[("Origin", ORIGIN), login, json], BODY), &config())), 404);
    assert_eq!(status(post(&[login, json], BODY)), 403, "no Origin: not a page");
    assert_eq!(status(post(&[("Origin", "https://evil.example"), login, json], BODY)), 403);
    assert_eq!(status(post(&[("Origin", ORIGIN), json], BODY)), 403, "no Tailscale login");
    assert_eq!(status(post(&[("Origin", ORIGIN), ("Tailscale-User-Login", "other@example.com"), json], BODY)), 403);
    assert_eq!(status(post(&[("Origin", ORIGIN), login, ("Content-Type", "text/plain")], BODY)), 415);
    assert_eq!(status(post(&[("Origin", ORIGIN), login, json], "{\"prompt\": 1}")), 400);
    assert_eq!(status(route(&request("GET", "/prompts", &[("Origin", ORIGIN), login], ""), &config())), 405);
}

#[test]
fn the_preflight_names_the_one_origin_method_and_header() {
    let Route::Respond(response) = route(&request("OPTIONS", "/prompts", &[("Origin", ORIGIN)], ""), &config()) else {
        panic!("a response");
    };
    assert_eq!(response.status, 204);
    for header in [
        ("Access-Control-Allow-Origin", ORIGIN),
        ("Access-Control-Allow-Methods", "POST"),
        ("Access-Control-Allow-Headers", "Content-Type"),
    ] {
        assert!(response.headers.contains(&(header.0.into(), header.1.into())), "{header:?}");
    }
}

#[test]
fn requests_are_read_up_to_their_length_and_no_further() {
    let raw = "POST /prompts HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nORIGIN: o\r\n\r\nhello, and more";
    let request = read_request(&mut raw.as_bytes()).expect("parsed");
    assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/prompts"));
    assert_eq!(request.header("origin"), Some("o"));
    assert_eq!(request.body, b"hello");
    let huge = format!("POST /prompts HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
    assert_eq!(read_request(&mut huge.as_bytes()).map_err(|r| r.status), Err(413));
    assert_eq!(read_request(&mut "POST / HTTP/1.1\r\n".as_bytes()).map_err(|r| r.status), Err(400));
}

#[test]
fn the_listener_waits_for_the_apps_answer() {
    let (tx, rx) = mpsc::channel();
    let listener = Listener::start(&config(), tx).expect("bound");
    let answering = listener.clone();
    std::thread::spawn(move || {
        if let Ok(Input::ApiPrompt { id, repository, prompt }) = rx.recv() {
            assert_eq!((repository.as_str(), prompt.as_str()), ("me/app", "Fix the build"));
            let accepted = Accepted { harness: "claude".into(), branch: "fix-build".into(), agent: "t-fix".into() };
            answering.answer(id, Ok(accepted));
        }
    });
    let mut stream = TcpStream::connect(("127.0.0.1", listener.port)).expect("connected");
    let head = format!(
        "POST /prompts HTTP/1.1\r\nOrigin: {ORIGIN}\r\nTailscale-User-Login: {LOGIN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        BODY.len()
    );
    stream.write_all(format!("{head}{BODY}").as_bytes()).expect("sent");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("answered");
    assert!(response.starts_with("HTTP/1.1 202 Accepted\r\n"), "{response}");
    assert!(response.contains(&format!("Access-Control-Allow-Origin: {ORIGIN}\r\n")), "{response}");
    assert!(response.ends_with(r#"{"harness":"claude","branch":"fix-build","agent":"t-fix"}"#), "{response}");
}

fn waiting(listener: &Listener, id: u64) -> mpsc::Receiver<Answer> {
    let (answer, rx) = mpsc::channel();
    listener.pending.lock().unwrap().insert(id, Slot { answer, claimed: false });
    rx
}

#[test]
fn a_prompt_given_up_on_is_never_claimed_and_a_claimed_one_is_awaited() {
    let listener = Listener { pending: Arc::default(), port: 0 };
    let _rx = waiting(&listener, 1);
    assert!(withdraw(&listener.pending, 1), "the request timed out first");
    assert!(!listener.claim(1), "so the app must not launch it");

    let rx = waiting(&listener, 2);
    assert!(listener.claim(2));
    assert!(!withdraw(&listener.pending, 2), "launching: the request waits on");
    let accepted = Accepted { harness: "codex".into(), branch: "b".into(), agent: "a".into() };
    listener.answer(2, Ok(accepted.clone()));
    assert_eq!(rx.try_recv(), Ok(Ok(accepted)));

    let rx = waiting(&listener, 3);
    listener.answer(3, Err(Refusal::invalid("no")));
    assert!(!withdraw(&listener.pending, 3), "answered just before the timeout");
    assert!(rx.try_recv().is_ok());
}

#[test]
fn refusals_carry_their_status_and_message() {
    let response = answered(Err(Refusal::not_found("No local checkout of me/app.")));
    assert_eq!(response.status, 404);
    assert_eq!(response.body, r#"{"error":"No local checkout of me/app."}"#);
}
