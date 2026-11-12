// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Parakeet as a child process.
//!
//! In sidecar mode Iago owns the server: it picks a free loopback port, starts
//! the binary with a random API key, waits for `/health`, and kills it on
//! shutdown. The binary and the models travel inside the Iago package, so the
//! user never installs Parakeet separately and never exposes it on the LAN.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::Settings;
use crate::error::{Error, Result};
use crate::paths::Paths;

/// Everything needed to launch one Parakeet process.
#[derive(Debug, Clone)]
pub struct SidecarSpec {
    /// Absolute path to the server binary.
    pub binary: PathBuf,
    /// Directory holding `asr/<precision>` and friends.
    pub models_dir: PathBuf,
    /// `int8` or `fp32`.
    pub precision: String,
    /// Loopback base URL the process will answer on.
    pub base_url: String,
    /// Bearer token required by the process.
    pub api_key: String,
}

impl SidecarSpec {
    /// Derives the launch plan from the user settings.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Result<Self> {
        // Always the copy Iago downloaded: a parakeet checkout on the machine is
        // a developer's, not the one this app is meant to run.
        let binary = paths.default_binary();

        if !binary.is_file() {
            return Err(Error::Transcription(format!(
                "the parakeet binary is missing at {}; run `iago models install`",
                binary.display()
            )));
        }

        let models_dir = if settings.asr.models_dir.is_empty() {
            paths.models_dir()
        } else {
            PathBuf::from(&settings.asr.models_dir)
        };

        let port = free_port()?;
        let api_key = if settings.asr.api_key.is_empty() {
            random_token()
        } else {
            settings.asr.api_key.clone()
        };

        Ok(Self {
            binary,
            models_dir,
            precision: settings.asr.precision.as_str().to_string(),
            base_url: format!("http://127.0.0.1:{port}"),
            api_key,
        })
    }

    /// TCP port encoded in [`Self::base_url`].
    pub fn port(&self) -> u16 {
        self.base_url
            .rsplit(':')
            .next()
            .and_then(|port| port.parse().ok())
            .unwrap_or(crate::config::DEFAULT_PARAKEET_PORT)
    }
}

/// A running Parakeet process owned by Iago.
pub struct Sidecar {
    child: Child,
    spec: SidecarSpec,
    /// Last lines the process printed, so a failed start can explain itself.
    log: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}

impl Sidecar {
    /// Starts the process. It is not usable until [`Self::wait_healthy`] passes.
    pub async fn spawn(spec: SidecarSpec) -> Result<Self> {
        let log = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        let mut command = Command::new(&spec.binary);
        command
            .arg("-port")
            .arg(spec.port().to_string())
            .arg("-models")
            .arg(&spec.models_dir)
            .arg("-model-precision")
            .arg(&spec.precision)
            .arg("-log-level")
            .arg("warn")
            // Accept long audio instead of rejecting it: dictation can be a
            // meeting recording just as easily as a sentence.
            .arg("-long-audio")
            .env("PARAKEET_API_KEY", &spec.api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|err| {
            Error::Transcription(format!("cannot start {}: {err}", spec.binary.display()))
        })?;

        // Drain stderr into the rolling log. Without a reader the pipe fills and
        // a chatty server blocks on write, which looks like it died for no reason;
        // and `last_log_lines` has something to say when the start fails.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(read_stderr(stderr, Arc::clone(&log)));
        }

        tracing::info!(
            binary = %spec.binary.display(),
            port = spec.port(),
            precision = %spec.precision,
            "parakeet sidecar started"
        );

        Ok(Self { child, spec, log })
    }

    /// Base URL of the running process.
    pub fn base_url(&self) -> &str {
        &self.spec.base_url
    }

    /// Polls `/health` until the model is loaded or `timeout` expires.
    pub async fn wait_healthy(&mut self, timeout: Duration) -> Result<()> {
        let client = crate::asr::ParakeetClient::new(
            self.spec.base_url.clone(),
            Some(self.spec.api_key.clone()),
            Duration::from_secs(5),
        )?;

        let deadline = Instant::now() + timeout;
        let mut last_error = String::from("no attempt made");

        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                let tail = self.last_log_lines(3);
                let tail = if tail.is_empty() {
                    String::new()
                } else {
                    format!(": {tail}")
                };
                return Err(Error::Transcription(format!(
                    "parakeet exited during start-up with {status}{tail}"
                )));
            }

            match client.health().await {
                Ok(()) => {
                    tracing::info!(port = self.spec.port(), "parakeet is ready");
                    return Ok(());
                }
                Err(err) => last_error = err.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        let tail = self.last_log_lines(3);
        let tail = if tail.is_empty() {
            String::new()
        } else {
            format!(" · {tail}")
        };

        Err(Error::Transcription(format!(
            "parakeet did not become healthy within {}s: {last_error}{tail}",
            timeout.as_secs()
        )))
    }

    /// Last lines the process printed, oldest first.
    pub fn last_log_lines(&self, count: usize) -> String {
        let Ok(log) = self.log.lock() else {
            return String::new();
        };

        let skip = log.len().saturating_sub(count);
        log.iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// Terminates the process, waiting briefly for a clean exit.
    pub async fn stop(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Most lines kept from the server's stderr; enough to explain a failed start.
const MAX_LOG_LINES: usize = 200;

/// Appends stderr lines to the rolling log, newest last.
async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    log: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
) {
    let mut lines = BufReader::new(stderr).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let mut log = log.lock().expect("stderr log lock poisoned");
        log.push_back(line);
        while log.len() > MAX_LOG_LINES {
            log.pop_front();
        }
    }
}

/// Asks the kernel for an unused loopback port.
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|err| Error::Transcription(format!("cannot reserve a local port: {err}")))?;
    let port = listener
        .local_addr()
        .map_err(|err| Error::Transcription(format!("cannot read the reserved port: {err}")))?
        .port();
    drop(listener);
    Ok(port)
}

/// Random bearer token, so a stray process on the same machine cannot use it.
fn random_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_reports_the_port_from_its_url() {
        let spec = SidecarSpec {
            binary: PathBuf::from("/bin/true"),
            models_dir: PathBuf::from("/tmp/models"),
            precision: "int8".to_string(),
            base_url: "http://127.0.0.1:47821".to_string(),
            api_key: "token".to_string(),
        };
        assert_eq!(spec.port(), 47821);
    }

    #[test]
    fn a_missing_binary_is_reported_before_spawning() {
        let settings = Settings::default();
        let paths = Paths::with_root("/tmp/iago-sidecar-test");
        let error = SidecarSpec::from_settings(&settings, &paths).expect_err("must fail");
        assert!(error.to_string().contains("parakeet binary is missing"));
    }
}
