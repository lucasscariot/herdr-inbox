//! Where a Herdr server listens, resolved the way the `herdr` CLI resolves it.
//!
//! Order: an explicit `--session`, then `HERDR_SOCKET_PATH`, then
//! `HERDR_SESSION`, then the default session. The config directory is
//! `$XDG_CONFIG_HOME/herdr` or `~/.config/herdr`.

use std::path::{Path, PathBuf};

/// The environment values that decide a socket path, captured once so the
/// resolution is a pure function that tests can drive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocketEnv {
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub socket_path: Option<PathBuf>,
    pub session: Option<String>,
}

impl SocketEnv {
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            home: var("HOME").map(PathBuf::from),
            xdg_config_home: var("XDG_CONFIG_HOME").map(PathBuf::from),
            socket_path: var("HERDR_SOCKET_PATH").map(PathBuf::from),
            session: var("HERDR_SESSION").and_then(|value| value.into_string().ok()),
        }
    }
}

/// The server a client talks to: its JSON API socket and the session name that
/// `herdr` subprocesses need to reach the same server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub api_socket: PathBuf,
    pub session: Option<String>,
}

/// Herdr's own limit for session names.
const MAX_SESSION_NAME_LEN: usize = 64;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SocketError {
    #[error("cannot find the Herdr config directory: neither XDG_CONFIG_HOME nor HOME is set")]
    NoConfigDir,
    #[error("invalid session name {0:?}: use letters, digits, '-', '_' or '.'")]
    InvalidSession(String),
}

pub fn config_dir(env: &SocketEnv) -> Result<PathBuf, SocketError> {
    if let Some(xdg) = &env.xdg_config_home {
        return Ok(xdg.join("herdr"));
    }
    env.home.as_ref().map(|home| home.join(".config").join("herdr")).ok_or(SocketError::NoConfigDir)
}

pub fn resolve(explicit_session: Option<&str>, env: &SocketEnv) -> Result<Endpoint, SocketError> {
    if let Some(session) = explicit_session {
        return session_endpoint(session, env);
    }
    if let Some(path) = &env.socket_path {
        return Ok(Endpoint { api_socket: path.clone(), session: session_from_socket(path) });
    }
    // Herdr silently ignores an invalid HERDR_SESSION, so do the same.
    if let Some(session) = env.session.as_deref().filter(|s| validate_session(s).is_ok()) {
        return session_endpoint(session, env);
    }
    Ok(Endpoint { api_socket: config_dir(env)?.join("herdr.sock"), session: None })
}

fn session_endpoint(session: &str, env: &SocketEnv) -> Result<Endpoint, SocketError> {
    validate_session(session)?;
    if session == "default" {
        return Ok(Endpoint { api_socket: config_dir(env)?.join("herdr.sock"), session: None });
    }
    Ok(Endpoint {
        api_socket: config_dir(env)?.join("sessions").join(session).join("herdr.sock"),
        session: Some(session.to_string()),
    })
}

fn validate_session(session: &str) -> Result<(), SocketError> {
    let valid = !session.is_empty()
        && session.len() <= MAX_SESSION_NAME_LEN
        && session != "."
        && session != ".."
        && session.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid { Ok(()) } else { Err(SocketError::InvalidSession(session.to_string())) }
}

/// `…/sessions/<name>/herdr.sock` names a session; anything else is either the
/// default session or a custom path that only `HERDR_SOCKET_PATH` can reach.
fn session_from_socket(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let sessions = parent.parent()?;
    if sessions.file_name()? != "sessions" || path.file_name()? != "herdr.sock" {
        return None;
    }
    parent.file_name()?.to_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> SocketEnv {
        SocketEnv { home: Some(PathBuf::from("/home/u")), ..SocketEnv::default() }
    }

    #[test]
    fn default_session_lives_in_the_config_dir() {
        let endpoint = resolve(None, &env()).unwrap();
        assert_eq!(endpoint.api_socket, PathBuf::from("/home/u/.config/herdr/herdr.sock"));
        assert_eq!(endpoint.session, None);
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        let env = SocketEnv { xdg_config_home: Some(PathBuf::from("/xdg")), ..env() };
        assert_eq!(resolve(None, &env).unwrap().api_socket, PathBuf::from("/xdg/herdr/herdr.sock"));
    }

    #[test]
    fn explicit_session_beats_socket_path_and_session_env() {
        let env = SocketEnv {
            socket_path: Some(PathBuf::from("/elsewhere/herdr.sock")),
            session: Some("from-env".into()),
            ..env()
        };
        let endpoint = resolve(Some("work"), &env).unwrap();
        assert_eq!(endpoint.api_socket, PathBuf::from("/home/u/.config/herdr/sessions/work/herdr.sock"));
        assert_eq!(endpoint.session.as_deref(), Some("work"));
    }

    #[test]
    fn socket_path_beats_session_env() {
        let env = SocketEnv {
            socket_path: Some(PathBuf::from("/elsewhere/herdr.sock")),
            session: Some("from-env".into()),
            ..env()
        };
        let endpoint = resolve(None, &env).unwrap();
        assert_eq!(endpoint.api_socket, PathBuf::from("/elsewhere/herdr.sock"));
        assert_eq!(endpoint.session, None);
    }

    #[test]
    fn socket_path_inside_sessions_dir_names_its_session() {
        let env = SocketEnv {
            socket_path: Some(PathBuf::from("/home/u/.config/herdr/sessions/inbox-test/herdr.sock")),
            ..env()
        };
        assert_eq!(resolve(None, &env).unwrap().session.as_deref(), Some("inbox-test"));
    }

    #[test]
    fn socket_path_with_another_file_name_names_no_session() {
        let env = SocketEnv { socket_path: Some(PathBuf::from("/x/sessions/a/custom.sock")), ..env() };
        assert_eq!(resolve(None, &env).unwrap().session, None);
    }

    #[test]
    fn session_env_is_used_when_nothing_else_is_set() {
        let env = SocketEnv { session: Some("night".into()), ..env() };
        assert_eq!(
            resolve(None, &env).unwrap().api_socket,
            PathBuf::from("/home/u/.config/herdr/sessions/night/herdr.sock")
        );
    }

    #[test]
    fn the_session_named_default_is_the_default_socket() {
        let endpoint = resolve(Some("default"), &env()).unwrap();
        assert_eq!(endpoint.api_socket, PathBuf::from("/home/u/.config/herdr/herdr.sock"));
        assert_eq!(endpoint.session, None);
    }

    #[test]
    fn session_names_cannot_escape_the_sessions_dir() {
        for bad in ["", ".", "..", "../x", "a/b", "a b", "é"] {
            assert_eq!(
                resolve(Some(bad), &env()),
                Err(SocketError::InvalidSession(bad.to_string())),
                "{bad:?} must be rejected"
            );
        }
        assert!(resolve(Some("a.b-c_9"), &env()).is_ok());
        assert!(resolve(Some(&"a".repeat(64)), &env()).is_ok());
        assert!(resolve(Some(&"a".repeat(65)), &env()).is_err());
    }

    #[test]
    fn an_invalid_session_env_falls_back_to_the_default_like_herdr() {
        let env = SocketEnv { session: Some("../evil".into()), ..env() };
        let endpoint = resolve(None, &env).unwrap();
        assert_eq!(endpoint.api_socket, PathBuf::from("/home/u/.config/herdr/herdr.sock"));
        assert_eq!(endpoint.session, None);
    }

    #[test]
    fn missing_home_and_xdg_is_an_error_not_a_relative_path() {
        assert_eq!(resolve(None, &SocketEnv::default()), Err(SocketError::NoConfigDir));
    }
}
