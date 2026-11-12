// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Error type shared by every layer.
//!
//! Every fallible boundary returns [`Result`] so callers can react to a
//! specific failure (no audio device, portal missing, Parakeet down) instead of
//! matching on strings.

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Every way Iago can fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The current platform (or build) cannot provide this capability.
    #[error("not supported here: {0}")]
    Unsupported(String),

    /// Invalid or unreadable configuration.
    #[error("configuration: {0}")]
    Config(String),

    /// Audio capture failed.
    #[error("audio: {0}")]
    Audio(String),

    /// Transcription failed.
    #[error("transcription: {0}")]
    Transcription(String),

    /// Delivering the text to the user failed.
    #[error("delivery: {0}")]
    Delivery(String),

    /// Hotkey registration failed.
    #[error("hotkeys: {0}")]
    Hotkey(String),

    /// Another Iago instance already owns the daemon socket.
    #[error("daemon: {0}")]
    Daemon(String),

    /// Generic I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// HTTP failure while talking to Parakeet.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// JSON (de)serialization failure, mostly the Parakeet response.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// TOML parse failure of the config file.
    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),

    /// TOML write failure of the config file.
    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),
}

impl Error {
    /// Convenience constructor for [`Error::Unsupported`].
    pub fn unsupported(what: impl Into<String>) -> Self {
        Self::Unsupported(what.into())
    }
}
