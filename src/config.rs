// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! User settings, persisted as TOML.
//!
//! The whole file is one [`Settings`] tree with one section per concern. Every
//! field has a default, so a partial (or missing) file still produces a valid
//! configuration. Unknown keys are rejected on purpose: a typo must fail loudly
//! instead of silently doing nothing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Default HTTP port of the bundled Parakeet server.
pub const DEFAULT_PARAKEET_PORT: u16 = 5092;

/// Default sample rate of the whole pipeline, and the only one Parakeet wants.
pub const DEFAULT_SAMPLE_RATE: u32 = 16_000;

/// Theme names the app accepts.
///
/// `auto` follows the desktop appearance. The other two are the only themes
/// that exist: the colours are fixed in code, not configurable.
pub const THEMES: [&str; 3] = ["auto", "mint", "mint-dark"];

/// Look and feel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeSettings {
    /// One of [`THEMES`].
    pub name: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            name: "auto".to_string(),
        }
    }
}

/// Root of the persisted configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Cross-cutting behaviour.
    pub general: GeneralSettings,
    /// How dictation is triggered.
    pub hotkey: HotkeySettings,
    /// Where the transcript goes.
    pub delivery: DeliverySettings,
    /// Audio capture.
    pub audio: AudioSettings,
    /// Speech recognition backend.
    pub asr: AsrSettings,
    /// Theme name. No colours here on purpose.
    pub theme: ThemeSettings,
}

/// General behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralSettings {
    /// Start the daemon when the user logs in.
    pub autostart: bool,
    /// Show a desktop notification when a dictation finishes.
    pub notifications: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            autostart: false,
            notifications: true,
        }
    }
}

/// How the shortcut behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HotkeyMode {
    /// Press to start, press again to stop.
    Toggle,
    /// Hold to record, release to transcribe.
    PushToTalk,
}

impl HotkeyMode {
    /// How the mode is written in the status and in the logs.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::PushToTalk => "push-to-talk",
        }
    }
}

/// Trigger settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeySettings {
    /// Combination as Iago stores it, e.g. `CTRL+SHIFT+d`.
    ///
    /// Each mechanism translates it: the portal and the shell want
    /// `<Control><Shift>d`, a compositor config wants its own spelling.
    pub binding: String,
    /// Toggle or hold.
    pub mode: HotkeyMode,
    /// Hard cap on a single recording, in seconds. The daemon stops and
    /// transcribes when it is reached, so a forgotten key does not record all
    /// afternoon.
    pub max_duration_secs: u32,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            binding: "SUPER+v".to_string(),
            mode: HotkeyMode::Toggle,
            max_duration_secs: 120,
        }
    }
}

/// Where the transcript goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryTarget {
    /// Copy it and stop there.
    Clipboard,
    /// Put it in the clipboard and paste it into whatever has the keyboard focus.
    ///
    /// `pointer` was an option and never worked: the synthetic click lands where
    /// the emulated pointer is, and Wayland does not let an app read where the
    /// real one is. Old files land here instead of refusing to load.
    #[serde(alias = "type", alias = "clipboard-and-type", alias = "pointer")]
    Focus,
}

/// What is appended after the transcript, so consecutive dictations flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AfterTranscript {
    /// Nothing, the next dictation continues the same word.
    Nothing,
    /// A space, the next dictation continues the same line.
    Space,
    /// A new line, the next dictation starts a new paragraph.
    Newline,
}

impl AfterTranscript {
    /// Text appended to the transcript.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Nothing => "",
            Self::Space => " ",
            Self::Newline => "\n",
        }
    }
}

/// Delivery settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliverySettings {
    /// Destination of the transcript.
    pub target: DeliveryTarget,
    /// What to append after the text.
    pub after: AfterTranscript,
}

impl Default for DeliverySettings {
    fn default() -> Self {
        Self {
            target: DeliveryTarget::Focus,
            after: AfterTranscript::Space,
        }
    }
}

/// Which capture backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioBackendKind {
    /// Pick the best one available on this platform.
    Auto,
    /// PipeWire capture.
    Pipewire,
    /// Synthetic silence, for testing the pipeline without a microphone.
    Mock,
}

/// Capture settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioSettings {
    /// Backend selection.
    pub backend: AudioBackendKind,
    /// PipeWire node name or id. Empty means the default source.
    pub device: String,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            backend: AudioBackendKind::Auto,
            device: String::new(),
        }
    }
}

/// Where the model lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsrMode {
    /// Iago owns the process: spawn, health check, keep it alive.
    Sidecar,
    /// Talk to a Parakeet instance we did not start (homelab, remote box).
    Remote,
}

/// Model precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Precision {
    /// 8-bit weights, ~670 MB on disk. The default.
    Int8,
    /// Full precision, ~2.4 GB on disk.
    Fp32,
}

impl Precision {
    /// Value accepted by `parakeet -model-precision` and used as directory name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Int8 => "int8",
            Self::Fp32 => "fp32",
        }
    }
}

/// Recognition settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AsrSettings {
    /// Sidecar or external server.
    pub mode: AsrMode,
    /// Base URL, used directly in remote mode.
    pub endpoint: String,
    /// Bearer token. Empty means "generate one per sidecar process".
    pub api_key: String,
    /// Extra request headers, one `Name: value` per line. Only used when talking
    /// to a remote server that needs more than a bearer token.
    pub headers: String,
    /// Model root. Empty means `<data dir>/models`.
    pub models_dir: String,
    /// Model precision to load.
    pub precision: Precision,
    /// ISO-639-1 hint. Empty lets the model decide.
    pub language: String,
    /// HTTP timeout for one transcription, in seconds.
    pub request_timeout_secs: u64,
}

impl Default for AsrSettings {
    fn default() -> Self {
        Self {
            mode: AsrMode::Sidecar,
            endpoint: format!("http://127.0.0.1:{DEFAULT_PARAKEET_PORT}"),
            api_key: String::new(),
            headers: String::new(),
            models_dir: String::new(),
            precision: Precision::Int8,
            language: String::new(),
            request_timeout_secs: 180,
        }
    }
}

impl Settings {
    /// Non-fatal problems worth telling the user about in the UI.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        if self.hotkey.binding.trim().is_empty() {
            warnings.push("the hotkey binding is empty".to_string());
        }
        if self.hotkey.max_duration_secs == 0 {
            warnings.push("max recording duration is 0, recordings never stop".to_string());
        }
        if self.asr.mode == AsrMode::Remote && self.asr.endpoint.trim().is_empty() {
            warnings.push("remote mode needs an endpoint".to_string());
        }
        if self.asr.request_timeout_secs == 0 {
            warnings.push("request timeout is 0, every transcription fails".to_string());
        }
        if !THEMES.contains(&self.theme.name.as_str()) {
            warnings.push(format!(
                "unknown theme {:?}, known ones are {}",
                self.theme.name,
                THEMES.join(", ")
            ));
        }

        warnings
    }
}

/// Thread-safe owner of the settings, backed by one TOML file.
#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    settings: RwLock<Settings>,
}

impl ConfigStore {
    /// Loads `path`, or creates it with defaults when it does not exist yet.
    pub fn load(path: impl Into<PathBuf>) -> Result<Arc<Self>> {
        let path = path.into();
        let settings = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            parse_settings(&raw)
                .map_err(|err| Error::Config(format!("cannot parse {}: {err}", path.display())))?
        } else {
            let defaults = Settings::default();
            write_file(&path, &defaults)?;
            defaults
        };

        Ok(Arc::new(Self {
            path,
            settings: RwLock::new(settings),
        }))
    }

    /// Path of the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consistent snapshot of the current settings.
    pub fn snapshot(&self) -> Settings {
        self.settings
            .read()
            .expect("settings lock poisoned")
            .clone()
    }

    /// Replaces the whole tree and persists it.
    pub fn replace(&self, settings: Settings) -> Result<()> {
        write_file(&self.path, &settings)?;
        *self.settings.write().expect("settings lock poisoned") = settings;
        Ok(())
    }

    /// Re-reads the file, replacing what is in memory.
    ///
    /// The daemon holds the store for its whole life while the UI writes the
    /// file on every edit, so without this the daemon would keep running with
    /// the settings it read at start-up.
    pub fn reload(&self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }

        let raw = std::fs::read_to_string(&self.path)?;
        let settings = parse_settings(&raw)
            .map_err(|err| Error::Config(format!("cannot parse {}: {err}", self.path.display())))?;

        *self.settings.write().expect("settings lock poisoned") = settings;
        Ok(())
    }

    /// Writes the file back from the current in-memory state.
    pub fn save(&self) -> Result<()> {
        let snapshot = self.snapshot();
        write_file(&self.path, &snapshot)
    }
}

/// Parses the file, dropping keys that this version no longer knows.
///
/// Settings get removed and renamed as the app matures. Rejecting the whole file
/// for one stale key would brick the app on upgrade, so the unknown keys are
/// pruned against the defaults and the rest is kept.
fn parse_settings(raw: &str) -> std::result::Result<Settings, toml::de::Error> {
    let first = match toml::from_str::<Settings>(raw) {
        Ok(settings) => return Ok(settings),
        Err(err) => err,
    };

    let Ok(mut value) = toml::from_str::<toml::Value>(raw) else {
        return Err(first);
    };
    let Ok(defaults) = toml::Value::try_from(Settings::default()) else {
        return Err(first);
    };

    if !prune_unknown(&mut value, &defaults) {
        return Err(first);
    }

    tracing::warn!("settings file had keys this version does not know; they were dropped");
    toml::from_str(&toml::to_string(&value).unwrap_or_default()).map_err(|_| first)
}

/// Removes keys from `value` that are not in `defaults`. Returns whether it
/// removed anything.
fn prune_unknown(value: &mut toml::Value, defaults: &toml::Value) -> bool {
    let (Some(table), Some(defaults)) = (value.as_table_mut(), defaults.as_table()) else {
        return false;
    };

    let mut removed = false;
    table.retain(|key, child| match defaults.get(key) {
        Some(default) => {
            if let Some(default) = default.as_table() {
                removed |= prune_unknown(child, &toml::Value::Table(default.clone()));
            }
            true
        }
        None => {
            removed = true;
            false
        }
    });

    removed
}

fn write_file(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(settings)?;
    let header = "# Iago settings. Written by the app; comments are not preserved.\n";
    std::fs::write(path, format!("{header}{raw}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Arc<ConfigStore>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let store = ConfigStore::load(&path).expect("load");
        (dir, store)
    }

    #[test]
    fn a_missing_file_is_created_with_defaults() {
        let (dir, store) = store();
        assert!(dir.path().join("config.toml").exists());
        assert_eq!(store.snapshot(), Settings::default());
    }

    #[test]
    fn a_roundtrip_preserves_every_field() {
        let (_dir, store) = store();
        let mut settings = Settings::default();
        settings.hotkey.binding = "SUPER+space".to_string();
        settings.asr.precision = Precision::Fp32;
        settings.delivery.target = DeliveryTarget::Clipboard;

        store.replace(settings.clone()).expect("replace");
        assert_eq!(store.snapshot(), settings);

        let reloaded = ConfigStore::load(store.path()).expect("reload");
        assert_eq!(reloaded.snapshot(), settings);
    }

    #[test]
    fn unknown_keys_are_dropped_and_the_rest_survives() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[general]\nnot_a_real_option = 1\nautostart = true\n",
        )
        .expect("write");

        let store = ConfigStore::load(&path).expect("a stale key must not brick the app");
        assert!(store.snapshot().general.autostart);
    }

    #[test]
    fn a_removed_setting_from_an_older_version_is_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        // `keep_last_transcript`, `restore_clipboard` and `idle_timeout_secs`
        // no longer exist.
        std::fs::write(
            &path,
            "[general]\nkeep_last_transcript = false\n\n[delivery]\nrestore_clipboard = true\ntarget = \"clipboard-and-type\"\n\n[asr]\nidle_timeout_secs = 600\n",
        )
        .expect("write");

        let store = ConfigStore::load(&path).expect("older config must still load");
        let settings = store.snapshot();

        // The old delivery name maps to the new one.
        assert_eq!(settings.delivery.target, DeliveryTarget::Focus);
    }

    #[test]
    fn warnings_flag_an_empty_binding() {
        let mut settings = Settings::default();
        settings.hotkey.binding = "   ".to_string();
        assert_eq!(settings.warnings().len(), 1);
    }
}
