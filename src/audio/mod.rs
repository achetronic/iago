// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Audio capture port.
//!
//! The rest of the app only sees [`AudioBackend`] and [`CaptureHandle`]. Each
//! platform picks an implementation in [`crate::platform`]:
//!
//! - Linux: [`pw_cat`], driving the official PipeWire client.
//! - macOS: `AVCaptureSession` behind the same trait (not written yet).
//!
//! Capture always runs on its own thread and delivers mono `f32` frames through
//! a channel, so the async side of the app never blocks on a device.

pub mod mock;
pub mod pw_cat;
pub mod sources;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::error::{Error, Result};

/// How many seconds of audio a single capture may hold before we refuse to grow
/// further. Guards against a stuck recorder eating memory.
const MAX_RECORDING_SECS: usize = 15 * 60;

/// Format of the samples coming out of a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpec {
    /// Samples per second.
    pub sample_rate: u32,
    /// Interleaved channels. Iago always asks for 1.
    pub channels: u16,
}

impl Default for AudioSpec {
    fn default() -> Self {
        Self {
            sample_rate: crate::config::DEFAULT_SAMPLE_RATE,
            channels: 1,
        }
    }
}

/// What a capture thread sends upstream.
#[derive(Debug)]
pub enum AudioEvent {
    /// A block of mono samples.
    Frame(Vec<f32>),
    /// The device stopped, either by request or because it disappeared.
    Ended,
}

/// Where audio comes from.
pub trait AudioBackend: Send + Sync {
    /// Stable identifier used in logs and in the UI.
    fn id(&self) -> &'static str;

    /// Format this backend produces.
    fn spec(&self) -> AudioSpec;

    /// Starts capturing. Returns once the stream is open.
    fn open(&self) -> Result<CaptureHandle>;
}

/// SIGTERM, so the client can flush instead of being killed outright.
#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, signal: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(pid, signal);
    }
}

/// A running capture.
pub struct CaptureHandle {
    spec: AudioSpec,
    events: Receiver<AudioEvent>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    child: Option<std::process::Child>,
}

impl CaptureHandle {
    /// Builds a handle from the pieces every backend produces.
    pub fn new(
        spec: AudioSpec,
        events: Receiver<AudioEvent>,
        stop_flag: Arc<AtomicBool>,
        worker: std::thread::JoinHandle<()>,
        child: Option<std::process::Child>,
    ) -> Self {
        Self {
            spec,
            events,
            stop_flag,
            worker: Some(worker),
            child,
        }
    }

    /// Format of the samples this capture yields.
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// Stops the capture and returns everything recorded, in order.
    ///
    /// Truncates at [`MAX_RECORDING_SECS`] worth of audio so a stuck device
    /// cannot exhaust memory.
    pub fn stop(mut self) -> Vec<f32> {
        self.stop_flag.store(true, Ordering::SeqCst);

        // Ask the client to stop and give it time to flush. Killing it outright
        // drops whatever it had not written yet, which is the end of the last
        // word, and that is audible in the transcript.
        if let Some(child) = self.child.as_mut() {
            #[cfg(unix)]
            {
                let pid = child.id() as i32;
                // SAFETY: `kill` only takes a pid and a signal number.
                unsafe {
                    libc_kill(pid, SIGTERM);
                }

                // It exits on SIGTERM, and the reader sees EOF after the last
                // byte, so waiting here is what preserves the tail.
                let deadline = std::time::Instant::now() + Duration::from_millis(600);
                while std::time::Instant::now() < deadline {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }

            let _ = child.kill();
            let _ = child.wait();
        }

        let limit = self.spec.sample_rate as usize * MAX_RECORDING_SECS;
        let mut samples = Vec::new();
        let mut idle = 0;

        while samples.len() < limit {
            match self.events.recv_timeout(Duration::from_millis(500)) {
                Ok(AudioEvent::Frame(mut frame)) => {
                    idle = 0;
                    samples.append(&mut frame);
                }
                Ok(AudioEvent::Ended) => break,
                Err(RecvTimeoutError::Timeout) => {
                    idle += 1;
                    if idle >= 4 {
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }

        samples.truncate(limit);
        samples
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Fails when a backend cannot run here, with a message the UI can show as is.
pub(crate) fn backend_missing(name: &str, hint: &str) -> Error {
    Error::Audio(format!("{name} is not available: {hint}"))
}

/// Root of every capture problem: no microphone, no permission, no daemon.
pub(crate) fn capture_failed(detail: impl std::fmt::Display) -> Error {
    Error::Audio(format!("cannot open the capture stream: {detail}"))
}
