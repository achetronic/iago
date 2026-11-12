// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The dictation pipeline: capture, encode, transcribe, deliver.
//!
//! Kept free of I/O policy so it can be exercised from tests and from the CLI
//! without a daemon, a portal or a running model.

use std::time::{Duration, Instant};

use crate::asr::wav;
use crate::audio::{AudioBackend, AudioSpec, CaptureHandle};
use crate::error::Result;

/// One in-flight recording.
pub struct Session {
    handle: CaptureHandle,
    spec: AudioSpec,
    started: Instant,
}

impl Session {
    /// Opens the backend and starts collecting frames.
    pub fn start(backend: &dyn AudioBackend) -> Result<Self> {
        let handle = backend.open()?;
        let spec = handle.spec();

        tracing::debug!(
            backend = backend.id(),
            sample_rate = spec.sample_rate,
            "capture started"
        );

        Ok(Self {
            handle,
            spec,
            started: Instant::now(),
        })
    }

    /// How long this recording has been running.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Format of the recorded samples.
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// Stops the capture and returns the samples. Blocking: run it off the
    /// async runtime.
    pub fn finish(self) -> Vec<f32> {
        self.handle.stop()
    }
}

/// Turns recorded samples into the WAV payload Parakeet expects.
pub fn encode(samples: &[f32], spec: AudioSpec) -> Vec<u8> {
    wav::encode(samples, spec.sample_rate, spec.channels)
}

/// Root-mean-square level, used to detect an empty recording.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Whether a recording is too quiet to be worth sending to the model.
pub fn is_silent(samples: &[f32], threshold: f32) -> bool {
    rms(samples) < threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_has_no_level() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 1024]), 0.0);
        assert!(is_silent(&[0.0; 1024], 0.001));
    }

    #[test]
    fn a_full_scale_signal_is_loud() {
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < f32::EPSILON);
        assert!(!is_silent(&[0.5; 128], 0.001));
    }
}
