// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Synthetic capture backend.
//!
//! Produces silence at the requested rate so the whole pipeline (capture, WAV
//! encoding, upload, delivery) can be exercised without a microphone: useful in
//! CI and while developing on a machine without audio hardware.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::error::Result;

use super::{AudioBackend, AudioEvent, AudioSpec, CaptureHandle};

/// Frame length in milliseconds.
const FRAME_MS: u64 = 20;

/// A backend that records exactly nothing.
#[derive(Debug, Clone, Default)]
pub struct MockBackend {
    spec: AudioSpec,
}

impl MockBackend {
    /// Creates a mock backend producing the given format.
    pub fn new(spec: AudioSpec) -> Self {
        Self { spec }
    }
}

impl AudioBackend for MockBackend {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn open(&self) -> Result<CaptureHandle> {
        let spec = self.spec;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);

        let frame_samples = (spec.sample_rate as u64 * FRAME_MS / 1000).max(1) as usize;

        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                let frame = vec![0.0; frame_samples];

                if tx.send(AudioEvent::Frame(frame)).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(FRAME_MS));
            }
            let _ = tx.send(AudioEvent::Ended);
        });

        Ok(CaptureHandle::new(spec, rx, stop, worker, None))
    }
}
