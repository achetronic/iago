// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Iago: private, local dictation for modern desktops.
//!
//! The crate is a library plus a thin binary, split in layers that only depend
//! downwards:
//!
//! ```text
//! cli ──> daemon ─> dictation ─> asr ─> (Parakeet over HTTP)
//!           │                     audio
//!           │                     input
//!           └─> ipc               platform  <- the only OS-aware layer
//!
//! The control center is a separate package (`ui/`) that speaks [`ipc`] and
//! links none of the platform machinery.
//! ```
//!
//! Rules that keep the port to macOS cheap:
//!
//! - Only [`platform`] may name an operating system.
//! - Everything else talks to [`audio::AudioBackend`], [`input::TextSink`],
//!   [`input::hotkey::HotkeyService`] and [`asr::Transcriber`].
//! - [`daemon`] is the single owner of mutable state. Clients go through
//!   [`ipc`].

pub mod ambassador;
pub mod asr;
pub mod audio;
pub mod autostart;
pub mod binary;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod dictation;
pub mod error;
pub mod input;
pub mod ipc;
pub mod logging;
pub mod models;
pub mod notify;
pub mod oci;
pub mod paths;
pub mod platform;
pub mod window;

/// Keeps the async-io reactor alive in this process.
///
/// The portal code speaks D-Bus through zbus's async-io backend, while the rest
/// of the daemon runs on Tokio. Both live in one process, so the reactor gets a
/// thread of its own. One call, at the start of the daemon and the ambassador.
pub fn start_async_io_reactor() {
    std::thread::Builder::new()
        .name("iago-async-io".to_string())
        .spawn(|| async_io::block_on(std::future::pending::<()>()))
        .ok();
}

pub use error::{Error, Result};

/// Version string shown in the UI and `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Reverse-DNS application id.
///
/// The XDG portals need it to identify the process: without an app id, GNOME
/// refuses to create a `GlobalShortcuts` session. It must match the desktop
/// entry installed from `packaging/`.
pub const APP_ID: &str = "com.achetronic.Iago";
