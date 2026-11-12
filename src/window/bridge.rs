// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Snapshot of what the daemon says, refreshed off the UI thread.
//!
//! The window never blocks: one thread polls the control socket and runs the
//! diagnostics, and the UI only reads a shared snapshot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{ConfigStore, Precision};
use crate::ipc::{self, Action as IpcAction, Request, StateView};
use crate::models::{self, ModelInfo};
use crate::oci;
use crate::paths::Paths;

/// How often the daemon is polled.
pub const POLL_INTERVAL: Duration = Duration::from_millis(700);

/// Everything the window renders.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Last phase reported by the daemon.
    pub status: Option<StateView>,
    /// Transport error, when the daemon cannot be reached.
    pub error: Option<String>,
    /// Whether the socket answered recently.
    pub online: bool,
    /// Models found on disk, recomputed after an install.
    pub models: Option<Vec<ModelInfo>>,
    /// State of the model download.
    pub install: InstallState,
    /// Parakeet server binary on disk.
    pub binary: Option<crate::binary::BinaryInfo>,
    /// State of the binary download.
    pub binary_install: InstallState,
    /// Capture sources of the system, read once at start-up.
    pub sources: Option<Vec<crate::audio::sources::Source>>,
    /// Health of the remote server, when remote mode is selected.
    pub remote_ok: Option<bool>,
}

/// Progress of the model download.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallState {
    /// A download is in flight.
    pub running: bool,
    /// What is being downloaded, for example `asr/int8`.
    pub stage: String,
    /// Bytes written in the current stage.
    pub bytes: u64,
    /// Total bytes of the current stage.
    pub total: u64,
    /// Error, when the download failed.
    pub error: Option<String>,
    /// Bytes written by the last successful download.
    pub finished_bytes: Option<u64>,
}

impl InstallState {
    /// Fraction of the current stage, between 0 and 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.bytes as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}

/// Handle owned by the window.
#[derive(Clone)]
pub struct StatusStore {
    shared: Arc<Mutex<Snapshot>>,
}

impl StatusStore {
    /// Current snapshot.
    pub fn get(&self) -> Snapshot {
        self.shared
            .lock()
            .map(|shared| shared.clone())
            .unwrap_or_default()
    }

    pub(crate) fn publish(&self, update: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut shared) = self.shared.lock() {
            update(&mut shared);
        }
    }
}

/// Starts the polling thread. Returns the handle the UI reads from.
pub fn spawn_poller(
    socket: PathBuf,
    models_root: PathBuf,
    precision: Precision,
    paths: Arc<Paths>,
    settings: Arc<ConfigStore>,
) -> StatusStore {
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let store = StatusStore {
        shared: Arc::clone(&shared),
    };

    std::thread::Builder::new()
        .name("iago-window-poll".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };

            let binary = crate::binary::inspect(&paths);
            if let Ok(mut shared) = shared.lock() {
                shared.binary = Some(binary);
            }

            let sources = crate::audio::sources::sources();
            if let Ok(mut shared) = shared.lock() {
                shared.sources = Some(sources);
            }

            let mut models = crate::models::inspect_with_images(&models_root, precision, &paths);
            runtime.block_on(models::fill_remote_digests(&mut models));
            if let Ok(mut shared) = shared.lock() {
                shared.models = Some(models);
            }

            let mut ticks: u32 = 0;
            loop {
                // The status page reports the remote server when that mode is
                // selected, so it needs the health of that endpoint too. Ten
                // seconds is cheap and enough.
                let settings = settings.snapshot();
                if settings.asr.mode == crate::config::AsrMode::Remote && ticks % 14 == 0 {
                    let reachable = runtime.block_on(async {
                        crate::asr::ParakeetClient::new(
                            settings.asr.endpoint.clone(),
                            crate::asr::optional(&settings.asr.api_key),
                            std::time::Duration::from_secs(5),
                        )
                        .map_err(|err| err.to_string())
                        .map(|client| async move { client.health().await.is_ok() })
                    });

                    let reachable = match reachable {
                        Ok(future) => runtime.block_on(future),
                        Err(_) => false,
                    };
                    if let Ok(mut shared) = shared.lock() {
                        shared.remote_ok = Some(reachable);
                    }
                }
                ticks = ticks.wrapping_add(1);

                let outcome =
                    runtime.block_on(ipc::call(&socket, &Request::new(IpcAction::DaemonStatus)));
                if let Ok(mut shared) = shared.lock() {
                    match outcome {
                        Ok(response) => {
                            shared.online = true;
                            shared.error = None;
                            if let Some(state) = response.state {
                                shared.status = Some(state);
                            }
                        }
                        Err(err) => {
                            shared.online = false;
                            shared.error = Some(err.to_string());
                        }
                    }
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        })
        .expect("cannot start the status poller");

    store
}

/// Runs one request in the background.
pub fn control(socket: PathBuf, action: IpcAction) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let _ = runtime.block_on(ipc::call(&socket, &Request::new(action)));
    });
}

/// Downloads every model, republishes the result and restarts the daemon's
/// engine so it picks them up. Runs off the UI thread and reports progress.
pub fn install_models(
    store: StatusStore,
    socket: PathBuf,
    settings: Arc<ConfigStore>,
    paths: Arc<Paths>,
) {
    if store.get().install.running {
        return;
    }

    store.publish(|shared| {
        shared.install = InstallState {
            running: true,
            ..InstallState::default()
        };
    });

    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        else {
            store.publish(|shared| {
                shared.install = InstallState {
                    running: false,
                    error: Some("cannot start the async runtime".to_string()),
                    ..InstallState::default()
                };
            });
            return;
        };

        let snapshot = settings.snapshot();
        let precision = snapshot.asr.precision;

        let sink_store = store.clone();
        let sink: oci::ProgressSink = Arc::new(move |stage: oci::Stage| {
            sink_store.publish(|shared| {
                shared.install.stage = stage.name;
                shared.install.bytes = stage.bytes;
                shared.install.total = stage.total;
            });
        });

        match runtime.block_on(models::install(&snapshot, &paths, Some(sink))) {
            Ok(report) => {
                let bytes: u64 = report.images.iter().map(|image| image.bytes).sum();
                let installed = report.models;
                store.publish(|shared| {
                    shared.models = Some(installed);
                    shared.install = InstallState {
                        finished_bytes: Some(bytes),
                        ..InstallState::default()
                    };
                });
                // New files on disk: restart the server so it loads them.
                control(socket.clone(), IpcAction::EngineRestart);
                control(socket, IpcAction::EngineStart);
            }
            Err(err) => {
                store.publish(|shared| {
                    shared.install = InstallState {
                        error: Some(err.to_string()),
                        ..InstallState::default()
                    };
                });
            }
        }

        let _ = precision;
    });
}

/// Downloads the Parakeet server, republishes it and restarts the engine.
pub fn install_binary(store: StatusStore, socket: PathBuf, paths: Arc<Paths>) {
    if store.get().binary_install.running {
        return;
    }

    store.publish(|shared| {
        shared.binary_install = InstallState {
            running: true,
            ..InstallState::default()
        };
    });

    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        else {
            store.publish(|shared| {
                shared.binary_install = InstallState {
                    running: false,
                    error: Some("cannot start the async runtime".to_string()),
                    ..InstallState::default()
                };
            });
            return;
        };

        let sink_store = store.clone();
        let sink: oci::ProgressSink = Arc::new(move |stage: oci::Stage| {
            sink_store.publish(|shared| {
                shared.binary_install.stage = stage.name;
                shared.binary_install.bytes = stage.bytes;
                shared.binary_install.total = stage.total;
            });
        });

        match runtime.block_on(crate::binary::install(&paths, Some(sink))) {
            Ok(info) => {
                store.publish(|shared| {
                    shared.binary = Some(info);
                    shared.binary_install = InstallState::default();
                });
                control(socket.clone(), IpcAction::EngineRestart);
                control(socket, IpcAction::EngineStart);
            }
            Err(err) => {
                store.publish(|shared| {
                    shared.binary_install = InstallState {
                        error: Some(err.to_string()),
                        ..InstallState::default()
                    };
                });
            }
        }
    });
}

/// Starts the model server and lets the daemon report the address.
pub fn stop_engine(socket: PathBuf) {
    control(socket, IpcAction::EngineStop);
}

pub fn start_engine(socket: PathBuf) {
    control(socket, IpcAction::EngineStart);
}
