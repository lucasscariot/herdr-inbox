//! Prompts from a web page on another device.
//!
//! Off unless `[api] enabled`. The listener binds 127.0.0.1 only;
//! `tailscale serve` publishes it to the tailnet over HTTPS and adds the
//! caller's login as `Tailscale-User-Login`. A request is accepted only from
//! the configured origin and login. `POST /prompts` with
//! `{"repository": "owner/name", "prompt": "…"}` goes to the app as an input,
//! which launches it exactly as the composer would, and answers with the
//! harness, branch and agent it chose.
//!
//! HTTP is kept to what one browser `fetch` needs: one request per
//! connection, `Content-Length` bodies, no keep-alive.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app::Input;
use crate::config::ApiConfig;

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Planning a launch is quick; the launch itself runs after the answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// A launch the app started for a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Accepted {
    pub harness: String,
    pub branch: String,
    pub agent: String,
}

/// Why a prompt started nothing, with the HTTP status that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
}

impl Refusal {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self { status: 422, message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: 404, message: message.into() }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self { status: 409, message: message.into() }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self { status: 503, message: message.into() }
    }
}

pub type Answer = Result<Accepted, Refusal>;

/// A request waiting for the app. Once claimed, the app is launching it, so
/// the request no longer gives up on it.
struct Slot {
    answer: Sender<Answer>,
    claimed: bool,
}

type Pending = Mutex<HashMap<u64, Slot>>;

/// The running listener. Answers come back from the event loop by id.
#[derive(Clone)]
pub struct Listener {
    pending: Arc<Pending>,
    /// The port bound, which differs from the config's only when that is 0.
    pub port: u16,
}

impl Listener {
    /// Binds the port and serves in the background. Fails when the port is
    /// taken, so a second inbox never answers for the first.
    pub fn start(config: &ApiConfig, tx: Sender<Input>) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))?;
        let port = listener.local_addr()?.port();
        let pending: Arc<Pending> = Arc::default();
        let counter = Arc::new(AtomicU64::new(0));
        let config = config.clone();
        let shared = pending.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (config, tx, pending, counter) = (config.clone(), tx.clone(), shared.clone(), counter.clone());
                thread::spawn(move || serve(stream, &config, &tx, &pending, &counter));
            }
        });
        Ok(Self { pending, port })
    }

    /// Takes prompt `id` for launching. False when its request already gave
    /// up: it was told so, and must not launch behind its back.
    pub fn claim(&self, id: u64) -> bool {
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        match pending.get_mut(&id) {
            Some(slot) => {
                slot.claimed = true;
                true
            }
            None => false,
        }
    }

    /// Hands the app's answer to the request waiting for it, if it still is.
    pub fn answer(&self, id: u64, answer: Answer) {
        let waiting = self.pending.lock().ok().and_then(|mut pending| pending.remove(&id));
        if let Some(waiting) = waiting {
            let _ = waiting.answer.send(answer);
        }
    }
}

/// Gives up on prompt `id` unless the app has claimed or answered it. True
/// when given up.
fn withdraw(pending: &Pending, id: u64) -> bool {
    let Ok(mut pending) = pending.lock() else {
        return false;
    };
    if pending.get(&id).is_none_or(|slot| slot.claimed) {
        return false;
    }
    pending.remove(&id);
    true
}

fn serve(mut stream: TcpStream, config: &ApiConfig, tx: &Sender<Input>, pending: &Pending, counter: &AtomicU64) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let response = match read_request(&mut stream) {
        // The header names the allowed page only, so it may read why its
        // request was refused while any other page still sees nothing.
        Err(response) => response.with_cors(config),
        Ok(request) => match route(&request, config) {
            Route::Respond(response) => response,
            Route::Prompt { repository, prompt } => {
                let id = counter.fetch_add(1, Ordering::Relaxed);
                let (answer_tx, answer_rx) = mpsc::channel();
                if let Ok(mut pending) = pending.lock() {
                    pending.insert(id, Slot { answer: answer_tx, claimed: false });
                }
                let late = || Err(Refusal::unavailable("Herdr Inbox did not answer in time."));
                let answer = if tx.send(Input::ApiPrompt { id, repository, prompt }).is_err() {
                    Err(Refusal::unavailable("Herdr Inbox is closing."))
                } else {
                    match answer_rx.recv_timeout(ANSWER_TIMEOUT) {
                        Ok(answer) => answer,
                        Err(_) if withdraw(pending, id) => late(),
                        // Claimed or answered just now: the launch is under
                        // way, so wait for it rather than report a failure.
                        Err(_) => answer_rx.recv_timeout(ANSWER_TIMEOUT).unwrap_or_else(|_| late()),
                    }
                };
                if let Ok(mut pending) = pending.lock() {
                    pending.remove(&id);
                }
                answered(answer).with_cors(config)
            }
        },
    };
    let _ = stream.write_all(&response.bytes());
    let _ = stream.flush();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Lower-cased names.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    fn json(status: u16, body: &impl Serialize) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_string(body).unwrap_or_else(|_| "{}".into()),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &serde_json::json!({ "error": message }))
    }

    /// Lets the allowed page read the answer.
    fn with_cors(mut self, config: &ApiConfig) -> Self {
        self.headers.push(("Access-Control-Allow-Origin".into(), config.allowed_origin.clone()));
        self.headers.push(("Vary".into(), "Origin".into()));
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Content",
            503 => "Service Unavailable",
            _ => "",
        };
        let mut head = format!("HTTP/1.1 {} {reason}\r\n", self.status);
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", self.body.len()));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(self.body.as_bytes());
        bytes
    }
}

fn read_request(stream: &mut impl Read) -> Result<Request, Response> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if buffer.len() > MAX_HEAD {
            return Err(Response::error(413, "Headers too large."));
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Response::error(400, "Incomplete request.")),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    };
    let mut request = parse_head(&buffer[..head_end]).ok_or_else(|| Response::error(400, "Malformed request."))?;
    let length = match request.header("content-length") {
        None => 0,
        Some(value) => value.trim().parse::<usize>().map_err(|_| Response::error(400, "Bad Content-Length."))?,
    };
    if length > MAX_BODY {
        return Err(Response::error(413, "Prompt too large."));
    }
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Response::error(400, "Incomplete body.")),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(length);
    request.body = body;
    Ok(request)
}

fn parse_head(head: &[u8]) -> Option<Request> {
    let head = std::str::from_utf8(head).ok()?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next()?.split(' ');
    let (method, path) = (start.next()?.to_string(), start.next()?.to_string());
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Some(Request { method, path, headers, body: Vec::new() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Respond(Response),
    Prompt { repository: String, prompt: String },
}

#[derive(Deserialize)]
struct PromptBody {
    repository: String,
    prompt: String,
}

/// Decides what a request gets. Pure, so every refusal is tested.
pub fn route(request: &Request, config: &ApiConfig) -> Route {
    use Route::Respond;
    if request.path.split('?').next() != Some("/prompts") {
        return Respond(Response::error(404, "Not found."));
    }
    // A browser always sends Origin on a cross-origin request: anything else
    // is another page, or no page at all.
    if request.header("origin") != Some(config.allowed_origin.as_str()) {
        return Respond(Response::error(403, "Origin not allowed."));
    }
    match request.method.as_str() {
        "OPTIONS" => {
            let mut response = Response { status: 204, headers: Vec::new(), body: String::new() }.with_cors(config);
            response.headers.push(("Access-Control-Allow-Methods".into(), "POST".into()));
            response.headers.push(("Access-Control-Allow-Headers".into(), "Content-Type".into()));
            response.headers.push(("Access-Control-Max-Age".into(), "600".into()));
            return Respond(response);
        }
        "POST" => {}
        _ => return Respond(Response::error(405, "Use POST.").with_cors(config)),
    }
    let login = request.header("tailscale-user-login").unwrap_or_default();
    if !login.eq_ignore_ascii_case(config.allowed_login.trim()) {
        return Respond(Response::error(403, "This Tailscale login may not send prompts.").with_cors(config));
    }
    let json = request.header("content-type").is_some_and(|t| t.split(';').next() == Some("application/json"));
    if !json {
        return Respond(Response::error(415, "Send JSON.").with_cors(config));
    }
    match serde_json::from_slice::<PromptBody>(&request.body) {
        Ok(body) => Route::Prompt { repository: body.repository.trim().to_string(), prompt: body.prompt },
        Err(_) => Respond(
            Response::error(400, "Expected {\"repository\": \"owner/name\", \"prompt\": \"…\"}.").with_cors(config),
        ),
    }
}

fn answered(answer: Answer) -> Response {
    match answer {
        Ok(accepted) => Response::json(202, &accepted),
        Err(refusal) => Response::error(refusal.status, &refusal.message),
    }
}

#[cfg(test)]
mod tests;
