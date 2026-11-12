// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Control protocol between the daemon and its clients.
//!
//! One request line, one response line, JSON, over a Unix socket in
//! `$XDG_RUNTIME_DIR`. That is enough for `iago ctl`, the control center and
//! future integrations, and it keeps the daemon the single owner of the
//! recording state.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::{Error, Result};

/// Protocol version. Bump when a field changes meaning, not when one is added.
/// One request from a client.
///
/// A single line of JSON with an `action`, plus room for per-action parameters
/// and for metadata about who is asking. Flat and additive on purpose: new
/// actions and new metadata do not break old clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Kind of event that produced this request: `command`, `push-to-talk`, and
    /// whatever comes next. Free-form on purpose, so a new gesture does not need a
    /// protocol change.
    #[serde(rename = "type", default = "default_kind")]
    pub kind: String,
    /// What to do.
    pub action: Action,
    /// Parameters for this action. Empty for the actions that take none.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Who is asking: the CLI, the control center, the desktop extension.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub meta: serde_json::Map<String, serde_json::Value>,
}

fn default_kind() -> String {
    KIND_COMMAND.to_string()
}

/// A request made by a person or a script, out of band.
pub const KIND_COMMAND: &str = "command";
/// A key being held down or let go.
pub const KIND_PUSH_TO_TALK: &str = "push-to-talk";

impl Request {
    /// A request with no parameters and no metadata.
    pub fn new(action: Action) -> Self {
        Self {
            kind: KIND_COMMAND.to_string(),
            action,
            params: serde_json::Map::new(),
            meta: serde_json::Map::new(),
        }
    }

    /// A request that comes from a gesture, such as a key being held.
    pub fn gesture(kind: &str, action: Action, source: &str) -> Self {
        let mut request = Self::new(action);
        request.kind = kind.to_string();
        request.meta.insert(
            "source".to_string(),
            serde_json::Value::String(source.to_string()),
        );
        request
    }

    /// A request that says where it came from.
    pub fn from_source(action: Action, source: &str) -> Self {
        let mut request = Self::new(action);
        request.meta.insert(
            "source".to_string(),
            serde_json::Value::String(source.to_string()),
        );
        request
    }
}

/// Everything a client can ask for.
///
/// The names are what the command line takes, so `iago ctl recording-toggle`
/// sends `{"action":"recording-toggle"}` and nobody has to translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Start or stop, depending on what the daemon is doing.
    RecordingToggle,
    /// Start recording. Fails when already recording.
    RecordingStart,
    /// Stop recording, transcribe and deliver.
    RecordingStop,
    /// The key of the shortcut went down.
    ShortcutPress,
    /// The key of the shortcut came up.
    ShortcutRelease,
    /// Phase, engine, shortcut and last transcript.
    DaemonStatus,
    /// Whether the daemon answers at all, with no state attached.
    DaemonAlive,
    /// Drop cached handles and reload the settings file.
    SettingsReload,
    /// Start the model server.
    EngineStart,
    /// Stop the model server.
    EngineStop,
    /// Stop and start the model server, so the next dictation loads new files.
    EngineRestart,
    /// Stop the daemon.
    DaemonShutdown,
}

/// Phase of the daemon, as seen by clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Nothing happening.
    Idle,
    /// Capturing audio.
    Recording,
    /// Waiting for the model.
    Transcribing,
    /// The last operation failed.
    Error,
}

impl Phase {
    /// Human readable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Recording => "Recording",
            Self::Transcribing => "Transcribing",
            Self::Error => "Error",
        }
    }
}

/// Snapshot of the daemon state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateView {
    /// Current phase.
    pub phase: Phase,
    /// Seconds spent in this phase.
    pub elapsed_secs: Option<u64>,
    /// Free-form detail: error text, engine status.
    pub detail: Option<String>,
    /// Last transcript, when the user asked to keep it.
    pub last_text: Option<String>,
    /// State of the model server.
    #[serde(default)]
    pub engine: EngineView,
    /// Who is delivering the shortcut right now.
    ///
    /// Live state, not configuration: the combination and the mode are settings,
    /// and the client that needs them reads them from the settings file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<ShortcutView>,
}

impl Default for StateView {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            elapsed_secs: None,
            detail: None,
            last_text: None,
            engine: EngineView::default(),
            shortcut: None,
        }
    }
}

/// Who owns the shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutView {
    /// Mechanism delivering it: `portal`, `desktop`, or whatever comes next.
    pub mechanism: String,
}

/// State of the model server, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineView {
    /// `stopped`, `starting`, `running` or `failed`.
    pub state: String,
    /// Address it listens on, or why it failed.
    pub detail: Option<String>,
}

impl Default for EngineView {
    fn default() -> Self {
        Self {
            state: "stopped".to_string(),
            detail: None,
        }
    }
}

impl EngineView {
    /// Short label for the status page.
    pub fn label(&self) -> &str {
        match self.state.as_str() {
            "running" => "listening",
            "starting" => "starting",
            "failed" => "failed",
            _ => "stopped",
        }
    }

    /// `true` when the server is answering.
    pub fn running(&self) -> bool {
        self.state == "running"
    }
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Done.
    Ok,
    /// Not done, and `message` says why.
    Error,
}

/// What the daemon answers.
///
/// One envelope for everything: how it ended, the state when the caller needs to
/// paint one, and a message when there is something to say. The minimal answer,
/// what a liveness check gets, is `{"status":"ok"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// How the request ended.
    pub status: Outcome,
    /// State after handling it, present when the caller has to paint one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateView>,
    /// Why it failed, or what happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Response {
    /// Something happened, and the state travels with it.
    pub fn ok(state: StateView) -> Self {
        Self {
            status: Outcome::Ok,
            state: Some(state),
            message: None,
        }
    }

    /// Done, and worth saying so.
    pub fn done(state: StateView, message: impl Into<String>) -> Self {
        Self {
            status: Outcome::Ok,
            state: Some(state),
            message: Some(message.into()),
        }
    }

    /// Not done. The state still travels, so the window can repaint.
    pub fn error(state: StateView, message: impl Into<String>) -> Self {
        Self {
            status: Outcome::Error,
            state: Some(state),
            message: Some(message.into()),
        }
    }

    /// The smallest answer that still respects the envelope.
    pub fn alive() -> Self {
        Self {
            status: Outcome::Ok,
            state: None,
            message: None,
        }
    }

    /// Whether the request was carried out.
    pub fn is_ok(&self) -> bool {
        self.status == Outcome::Ok
    }
}

/// Sends one request and reads the answer.
pub async fn call(socket: &Path, request: &Request) -> Result<Response> {
    let stream = UnixStream::connect(socket).await.map_err(|err| {
        Error::Daemon(format!(
            "cannot reach the daemon on {}: {err}; start it with `iago daemon run`",
            socket.display()
        ))
    })?;

    exchange(stream, request).await
}

/// Happy path of one conversation, shared by client and server.
pub async fn exchange(stream: UnixStream, request: &Request) -> Result<Response> {
    let (read_half, mut write_half) = stream.into_split();

    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');
    write_half.write_all(payload.as_bytes()).await?;
    write_half.flush().await?;

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Err(Error::Daemon(
            "the daemon closed the connection".to_string(),
        ));
    }

    let response = serde_json::from_str(&line)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_roundtrip_through_json() {
        for action in [
            Action::RecordingToggle,
            Action::RecordingStart,
            Action::RecordingStop,
            Action::ShortcutPress,
            Action::ShortcutRelease,
            Action::DaemonStatus,
            Action::DaemonAlive,
            Action::SettingsReload,
            Action::EngineStart,
            Action::EngineStop,
            Action::EngineRestart,
            Action::DaemonShutdown,
        ] {
            let request = Request::from_source(action, "test");
            let encoded = serde_json::to_string(&request).expect("encode");
            let decoded: Request = serde_json::from_str(&encoded).expect("decode");
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn a_request_names_its_action_and_says_who_asked() {
        let encoded =
            serde_json::to_string(&Request::from_source(Action::RecordingToggle, "shortcut"))
                .expect("encode");

        assert_eq!(
            encoded,
            r#"{"type":"command","action":"recording-toggle","meta":{"source":"shortcut"}}"#
        );

        let decoded: Request = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded.action, Action::RecordingToggle);
        assert_eq!(decoded.kind, KIND_COMMAND);
    }

    #[test]
    fn a_gesture_carries_its_type() {
        let encoded = serde_json::to_string(&Request::gesture(
            KIND_PUSH_TO_TALK,
            Action::ShortcutPress,
            "portal",
        ))
        .expect("encode");

        assert_eq!(
            encoded,
            r#"{"type":"push-to-talk","action":"shortcut-press","meta":{"source":"portal"}}"#
        );
    }

    #[test]
    fn the_smallest_answer_is_the_envelope_alone() {
        assert_eq!(
            serde_json::to_string(&Response::alive()).expect("encode"),
            r#"{"status":"ok"}"#
        );
    }

    #[test]
    fn a_full_answer_carries_the_state_and_a_message() {
        let response = Response::done(StateView::default(), "settings reloaded");
        let encoded = serde_json::to_string(&response).expect("encode");

        assert!(encoded.starts_with(r#"{"status":"ok","state":{"#));
        assert!(encoded.contains(r#""message":"settings reloaded""#));
    }

    #[test]
    fn a_failure_says_so_and_keeps_the_state() {
        let response = Response::error(StateView::default(), "the recording was silent");

        assert!(!response.is_ok());
        assert!(response.state.is_some());
        assert_eq!(
            response.message.as_deref(),
            Some("the recording was silent")
        );
    }
}
