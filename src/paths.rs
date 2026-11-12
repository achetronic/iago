// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Where Iago keeps its files.
//!
//! Everything is XDG-shaped on Linux and maps to the platform equivalents on
//! macOS through the `dirs` crate, so no layer above needs to know
//! about `~/.config` or `%APPDATA%`.
//!
//! ```text
//! $XDG_CONFIG_HOME/iago/config.toml   settings
//! $XDG_DATA_HOME/iago/models/         exported ONNX assets + parakeet binary
//! $XDG_DATA_HOME/iago/logs/           optional log files
//! $XDG_CACHE_HOME/iago/               scratch space
//! $XDG_RUNTIME_DIR/iago/daemon.sock   control socket (never persisted)
//! ```

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable that overrides the config/data root, useful for tests
/// and for running several profiles side by side.
const ROOT_ENV: &str = "IAGO_HOME";

/// Resolved directory layout for one user.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Directory holding `config.toml`.
    pub config_dir: PathBuf,
    /// Directory holding models, logs and the bundled Parakeet binary.
    pub data_dir: PathBuf,
    /// Scratch space, safe to delete at any time.
    pub cache_dir: PathBuf,
    /// Per-session directory for the control socket.
    pub runtime_dir: PathBuf,
}

impl Paths {
    /// Resolves the layout, honouring `IAGO_HOME` when it is set.
    pub fn resolve() -> Result<Self> {
        // An empty value counts as unset: a shell that exports `IAGO_HOME=` would
        // otherwise resolve every path relative to the working directory.
        if let Some(root) = std::env::var_os(ROOT_ENV) {
            if !root.is_empty() {
                return Ok(Self::with_root(PathBuf::from(root)));
            }
        }

        let config_dir = dirs::config_dir()
            .ok_or_else(|| Error::Config("cannot resolve the user config directory".into()))?
            .join("iago");
        let data_dir = dirs::data_dir()
            .ok_or_else(|| Error::Config("cannot resolve the user data directory".into()))?
            .join("iago");
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| Error::Config("cannot resolve the user cache directory".into()))?
            .join("iago");
        let runtime_dir = runtime_root(&data_dir);

        Ok(Self {
            config_dir,
            data_dir,
            cache_dir,
            runtime_dir,
        })
    }

    /// Layout rooted at an arbitrary directory. Used by `IAGO_HOME` and tests.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            runtime_dir: runtime_root(&root),
        }
    }

    /// `config.toml` inside the config directory.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Cache of the model images pulled, next to the settings file.
    pub fn images_file(&self) -> PathBuf {
        self.config_dir.join("images.json")
    }

    /// Root of the model tree (`asr/int8`, `vad`, `diarization/int8`, ...).
    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    /// Default location for the bundled `parakeet` server binary.
    pub fn default_binary(&self) -> PathBuf {
        self.data_dir.join("bin").join(binary_name())
    }

    /// Directory for log files.
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// Log file of one binary, e.g. `logs/iago.log`.
    pub fn log_file(&self, name: &str) -> PathBuf {
        self.logs_dir().join(format!("{name}.log"))
    }

    /// Control socket used by `iago ctl` and the UI to reach the daemon.
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir.join("daemon.sock")
    }

    /// Creates every directory that must exist before the app can run.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.runtime_dir,
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|err| Error::Config(format!("cannot create {}: {err}", dir.display())))?;
        }
        Ok(())
    }
}

/// Runtime directory: `$XDG_RUNTIME_DIR` when present, otherwise a per-user
/// path under the temp directory so the socket never lands somewhere shared.
fn runtime_root(fallback_base: &Path) -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("iago"),
        _ => fallback_base.join("run"),
    }
}

/// Name of the bundled Parakeet server next to the models.
pub const fn binary_name() -> &'static str {
    "parakeet"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_override_keeps_everything_under_it() {
        let paths = Paths::with_root("/tmp/iago-test-root");

        assert_eq!(
            paths.config_file(),
            Path::new("/tmp/iago-test-root/config/config.toml")
        );
        assert_eq!(
            paths.models_dir(),
            Path::new("/tmp/iago-test-root/data/models")
        );
        assert!(paths.socket().ends_with("daemon.sock"));
    }
}
