// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Speech recognition port.
//!
//! [`Transcriber`] is the only thing the pipeline knows about. Today the sole
//! implementation is [`parakeet::ParakeetClient`], talking to the Parakeet HTTP
//! API either on a process we own ([`sidecar`]) or on a remote box.
//!
//! Speaker diarization (`diarize=true`, up to 8 speakers) already exists on the
//! server side. It is deliberately unused: it belongs to the conversation
//! history feature, not to dictation.

pub mod parakeet;
pub mod sidecar;
pub mod wav;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::{AsrMode, ConfigStore};
use crate::error::{Error, Result};
use crate::paths::Paths;

pub use parakeet::ParakeetClient;
pub use sidecar::Sidecar;

/// What the model returned for one recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// The text. Empty when the model heard only silence.
    pub text: String,
    /// Detected or requested language, when the server reports it.
    pub language: Option<String>,
    /// Audio duration reported by the server, in seconds.
    pub duration_secs: Option<f64>,
}

impl Transcript {
    /// `true` when the model produced no usable text.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Anything that turns audio into text.
#[async_trait]
pub trait Transcriber: Send + Sync {
    /// Stable identifier used in logs and the UI.
    fn id(&self) -> &'static str;

    /// Cheap liveness probe.
    async fn health(&self) -> Result<()>;

    /// Transcribes a complete WAV payload.
    async fn transcribe(&self, wav: Vec<u8>, language: Option<&str>) -> Result<Transcript>;
}

/// One transcription backend available to the pipeline.
pub enum Engine {
    /// Parakeet over HTTP, possibly under our own process.
    Parakeet(ParakeetEngine),
}

impl Engine {
    /// Builds the engine described by the settings.
    pub fn from_settings(settings: Arc<ConfigStore>, paths: Arc<Paths>) -> Self {
        Self::Parakeet(ParakeetEngine::new(settings, paths))
    }

    /// Boots the backend and returns the address it listens on.
    pub async fn start(&self) -> Result<String> {
        match self {
            Self::Parakeet(engine) => engine.start().await,
        }
    }

    /// Transcribes one recording.
    pub async fn transcribe(&self, wav: Vec<u8>, language: Option<&str>) -> Result<Transcript> {
        match self {
            Self::Parakeet(engine) => engine.transcribe(wav, language).await,
        }
    }

    /// Human readable state for the UI and the status request.
    pub async fn status(&self) -> EngineStatus {
        match self {
            Self::Parakeet(engine) => engine.status().await,
        }
    }

    /// Stops any process we started.
    pub async fn shutdown(&self) {
        match self {
            Self::Parakeet(engine) => engine.shutdown().await,
        }
    }

    /// Exit status of the sidecar when it died on its own.
    ///
    /// `None` means it is either alive or was stopped on purpose; the watcher
    /// tells those apart by looking only at processes nobody killed.
    pub async fn sidecar_exit(&self) -> Option<std::process::ExitStatus> {
        match self {
            Self::Parakeet(engine) => engine.sidecar_exit().await,
        }
    }
}

/// State of the recognition backend, as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Nothing started yet.
    Stopped,
    /// Remote mode; nothing to start.
    Remote(String),
    /// Sidecar running and answering.
    Running(String),
    /// Something failed, with the reason.
    Failed(String),
}

/// Parakeet-backed engine. Owns the sidecar when we are in sidecar mode.
pub struct ParakeetEngine {
    settings: Arc<ConfigStore>,
    paths: Arc<Paths>,
    state: tokio::sync::Mutex<Option<Booted>>,
}

struct Booted {
    client: Arc<ParakeetClient>,
    sidecar: Option<Sidecar>,
    base_url: String,
}

impl ParakeetEngine {
    /// Creates an engine. Nothing is spawned until the first transcription.
    pub fn new(settings: Arc<ConfigStore>, paths: Arc<Paths>) -> Self {
        Self {
            settings,
            paths,
            state: tokio::sync::Mutex::new(None),
        }
    }

    /// Boots the sidecar (or checks the remote) and reports the address.
    async fn start(&self) -> Result<String> {
        let guard = self.ensure_client().await?;
        Ok(guard.base_url().to_string())
    }

    async fn transcribe(&self, wav: Vec<u8>, language: Option<&str>) -> Result<Transcript> {
        let client = self.ensure_client().await?;
        let configured = self.settings.snapshot().asr.language;
        let language = language
            .map(str::to_string)
            .filter(|hint| !hint.is_empty())
            .or_else(|| Some(configured).filter(|hint| !hint.is_empty()));
        match client.transcribe(wav, language.as_deref()).await {
            Ok(transcript) => Ok(transcript),
            Err(err) => {
                // A dead or wedged server would keep failing forever: forget it,
                // so the next dictation starts a fresh one.
                tracing::warn!(error = %err, "transcription failed, dropping the server");
                self.shutdown().await;
                Err(err)
            }
        }
    }

    async fn status(&self) -> EngineStatus {
        let snapshot = self.settings.snapshot();
        match snapshot.asr.mode {
            AsrMode::Remote => EngineStatus::Remote(snapshot.asr.endpoint),
            AsrMode::Sidecar => {
                let guard = self.state.lock().await;
                match guard.as_ref() {
                    Some(booted) => EngineStatus::Running(booted.base_url.clone()),
                    None => EngineStatus::Stopped,
                }
            }
        }
    }

    async fn shutdown(&self) {
        let mut guard = self.state.lock().await;
        if let Some(mut booted) = guard.take() {
            if let Some(sidecar) = booted.sidecar.as_mut() {
                sidecar.stop().await;
            }
        }
    }

    /// Exit status of the owned sidecar, when it ended by itself.
    async fn sidecar_exit(&self) -> Option<std::process::ExitStatus> {
        let mut guard = self.state.lock().await;
        let booted = guard.as_mut()?;
        let sidecar = booted.sidecar.as_mut()?;
        match sidecar.exit_status() {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the parakeet exit status");
                None
            }
        }
    }

    /// Returns a client, booting the sidecar on first use.
    async fn ensure_client(&self) -> Result<Arc<ParakeetClient>> {
        let mut guard = self.state.lock().await;
        if let Some(booted) = guard.as_ref() {
            return Ok(Arc::clone(&booted.client));
        }

        let snapshot = self.settings.snapshot();
        let timeout = Duration::from_secs(snapshot.asr.request_timeout_secs.max(1));

        let booted = match snapshot.asr.mode {
            AsrMode::Remote => {
                let client = ParakeetClient::with_headers(
                    snapshot.asr.endpoint.clone(),
                    optional(&snapshot.asr.api_key),
                    timeout,
                    parse_headers(&snapshot.asr.headers),
                )?;
                client.health().await?;
                Booted {
                    client: Arc::new(client),
                    sidecar: None,
                    base_url: snapshot.asr.endpoint.clone(),
                }
            }
            AsrMode::Sidecar => {
                let spec = sidecar::SidecarSpec::from_settings(&snapshot, &self.paths)?;
                let base_url = spec.base_url.clone();
                let mut sidecar = Sidecar::spawn(spec.clone()).await?;
                sidecar.wait_healthy(Duration::from_secs(90)).await?;

                let client =
                    ParakeetClient::new(base_url.clone(), Some(spec.api_key.clone()), timeout)?;
                Booted {
                    client: Arc::new(client),
                    sidecar: Some(sidecar),
                    base_url,
                }
            }
        };

        let client = Arc::clone(&booted.client);
        *guard = Some(booted);
        Ok(client)
    }
}

/// Wraps a failure with the backend name, so logs are unambiguous.
/// Parses `Name: value` lines into request headers.
///
/// Lines that do not look like a header are skipped with a warning: a typo in
/// the config must not stop transcription.
pub fn parse_headers(raw: &str) -> Vec<(String, String)> {
    let mut headers = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            tracing::warn!(line, "ignoring a header that is not `Name: value`");
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        headers.push((name.to_string(), value.to_string()));
    }

    headers
}

/// Empty config values mean "unset"; the client wants an `Option`.
pub fn optional(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn backend_error(detail: impl std::fmt::Display) -> Error {
    Error::Transcription(format!("parakeet: {detail}"))
}
