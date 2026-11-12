// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Parakeet HTTP client.
//!
//! Speaks the OpenAI-compatible surface of the server: `GET /health` and
//! `POST /v1/audio/transcriptions` with a multipart body. Nothing else is
//! needed for dictation.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::asr::{Transcriber, Transcript, backend_error};
use crate::error::{Error, Result};

/// Model name the server accepts. It ignores the value, but the field is part
/// of the OpenAI contract and some proxies are strict about it.
const MODEL_ALIAS: &str = "whisper-1";

/// Talks to one Parakeet endpoint.
#[derive(Debug)]
pub struct ParakeetClient {
    base_url: String,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
    http: reqwest::Client,
}

impl ParakeetClient {
    /// Builds a client for `base_url`, e.g. `http://127.0.0.1:5092`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Result<Self> {
        Self::with_headers(base_url, api_key, timeout, Vec::new())
    }

    /// Same, with extra request headers for servers that need more than a token.
    pub fn with_headers(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
        headers: Vec<(String, String)>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| backend_error(format!("cannot build the HTTP client: {err}")))?;

        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.filter(|key| !key.is_empty()),
            headers,
            http,
        })
    }

    /// Base URL without the trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .request(method, format!("{}{path}", self.base_url));

        if let Some(key) = self.api_key.as_deref() {
            builder = builder.bearer_auth(key);
        }

        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }

        builder
    }

    /// Checks that the server is up and authenticated.
    pub async fn health(&self) -> Result<()> {
        let response = self
            .request(reqwest::Method::GET, "/health")
            .send()
            .await
            .map_err(|err| backend_error(format!("health check failed: {err}")))?;

        if !response.status().is_success() {
            return Err(backend_error(format!(
                "health check returned {}",
                response.status()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Transcriber for ParakeetClient {
    fn id(&self) -> &'static str {
        "parakeet"
    }

    async fn health(&self) -> Result<()> {
        ParakeetClient::health(self).await
    }

    async fn transcribe(&self, wav: Vec<u8>, language: Option<&str>) -> Result<Transcript> {
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|err| backend_error(format!("cannot build the request body: {err}")))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", MODEL_ALIAS)
            .text("response_format", "json");

        if let Some(language) = language {
            form = form.text("language", language.to_string());
        }

        let response = self
            .request(reqwest::Method::POST, "/v1/audio/transcriptions")
            .multipart(form)
            .send()
            .await
            .map_err(|err| backend_error(format!("request failed: {err}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| backend_error(format!("cannot read the response: {err}")))?;

        if !status.is_success() {
            return Err(backend_error(format!(
                "server returned {status}: {}",
                body.trim()
            )));
        }

        let parsed: TranscriptionResponse = serde_json::from_str(&body)
            .map_err(|err| backend_error(format!("unexpected response shape: {err}")))?;

        Ok(Transcript {
            text: parsed.text.trim().to_string(),
            language: parsed.language,
            duration_secs: parsed.duration,
        })
    }
}

/// JSON body of `/v1/audio/transcriptions` with `response_format=json`.
#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

/// Rejects a payload the server would reject anyway, with a clearer message.
pub fn validate_payload(wav: &[u8]) -> Result<()> {
    if wav.len() <= 44 {
        return Err(Error::Transcription(
            "the recording is too short to transcribe".to_string(),
        ));
    }
    Ok(())
}
