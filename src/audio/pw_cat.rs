// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! PipeWire capture through the official `pw-cat` client.
//!
//! `pw-cat --record --raw` streams mono `f32` PCM to stdout, which is the
//! smallest useful contract with PipeWire and needs no C headers at build time.
//!
//! Why not the `pipewire` crate yet: it links against `libpipewire-0.3` and its
//! headers, which would force every contributor and every CI image to install
//! the development package. The trait boundary means swapping this file for the
//! native bindings later touches nothing else. That swap is the next audio step
//! documented in `agents/ARCHITECTURE.md`.
//!
//! What we keep from the native API by going through the CLI: node targeting
//! (`--target`), channel and rate negotiation, and PipeWire's own resampler.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::error::Result;

use super::{AudioBackend, AudioEvent, AudioSpec, CaptureHandle, backend_missing, capture_failed};

/// PipeWire client binary used for capture.
const CLIENT: &str = "pw-cat";

/// Bytes per `f32` sample.
const SAMPLE_BYTES: usize = 4;

/// Bytes read from the client per iteration.
const READ_CHUNK: usize = 8 * 1024;

/// Capture backend bound to PipeWire.
#[derive(Debug, Clone)]
pub struct PipewireBackend {
    spec: AudioSpec,
    device: Option<String>,
}

impl PipewireBackend {
    /// Creates a backend for the given format and optional target node.
    pub fn new(spec: AudioSpec, device: Option<String>) -> Self {
        Self { spec, device }
    }

    /// Whether the PipeWire client is on `PATH`.
    pub fn available() -> bool {
        which(CLIENT).is_some()
    }
}

impl AudioBackend for PipewireBackend {
    fn id(&self) -> &'static str {
        "pipewire"
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn open(&self) -> Result<CaptureHandle> {
        if !Self::available() {
            return Err(backend_missing(CLIENT, "install pipewire-utils"));
        }

        let mut command = Command::new(CLIENT);
        command
            .arg("--record")
            .arg("--raw")
            .arg("--format")
            .arg("f32")
            .arg("--rate")
            .arg(self.spec.sample_rate.to_string())
            .arg("--channels")
            .arg(self.spec.channels.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(device) = self.device.as_deref() {
            command.arg("--target").arg(device);
        }
        command.arg("-");

        let mut child = command
            .spawn()
            .map_err(|err| capture_failed(format!("cannot start {CLIENT}: {err}")))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| capture_failed("the client produced no output stream"))?;

        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let worker = std::thread::spawn(move || {
            use std::io::Read;

            let mut raw = vec![0u8; READ_CHUNK];
            let mut carry: Vec<u8> = Vec::with_capacity(SAMPLE_BYTES);

            loop {
                let read = match stdout.read(&mut raw) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(_) => break,
                };
                carry.extend_from_slice(&raw[..read]);

                let whole = carry.len() - carry.len() % SAMPLE_BYTES;
                if whole == 0 {
                    continue;
                }
                let frame: Vec<f32> = carry[..whole]
                    .chunks_exact(SAMPLE_BYTES)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                carry.drain(..whole);

                if tx.send(AudioEvent::Frame(frame)).is_err() {
                    break;
                }
            }

            let _ = tx.send(AudioEvent::Ended);
        });

        Ok(CaptureHandle::new(self.spec, rx, stop, worker, Some(child)))
    }
}

/// Minimal `PATH` lookup so the crate avoids an extra dependency.
pub(crate) fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}
