// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The only layer allowed to know which OS we run on.
//!
//! Everything above talks to [`AudioBackend`], [`TextSink`] and [`HotkeyService`].
//! Adding macOS means writing a new file here and nothing else:
//!
//! ```text
//! platform/mod.rs   trait Platform + runtime selection
//! platform/linux.rs PipeWire capture, portal hotkeys, enigo/libei delivery
//! platform/macos.rs stubs + the plan (AVCaptureSession, CGEventTap, NSPasteboard)
//! ```
//!
//! [`Capabilities`] is what the UI shows, so an unsupported
//! platform degrades into a clear message instead of a crash.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use crate::audio::AudioBackend;
use crate::config::{AudioSettings, DeliverySettings};
use crate::error::Result;
use crate::input::TextSink;
use crate::input::hotkey::{HotkeyBinding, HotkeyCapabilities, HotkeyService};

/// What this platform can actually do, for the UI and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// `linux` or `macos`.
    pub platform: &'static str,
    /// Clipboard read and write.
    pub clipboard: bool,
    /// How text is typed into the focused window.
    pub text_injection: &'static str,
    /// How the shortcut is delivered.
    pub global_hotkeys: &'static str,
    /// How audio is captured.
    pub audio: &'static str,
}

/// Every OS-specific capability Iago needs.
pub trait Platform: Send + Sync {
    /// `linux` or `macos`.
    fn id(&self) -> &'static str;

    /// Static capabilities of this build and session.
    fn capabilities(&self) -> Capabilities;

    /// Capture backend matching the settings.
    fn audio_backend(&self, settings: &AudioSettings) -> Result<Box<dyn AudioBackend>>;

    /// Delivery path matching the settings.
    fn text_sink(&self, settings: &DeliverySettings) -> Result<Box<dyn TextSink>>;

    /// Hotkey service for this session.
    ///
    /// The implementation chooses the mechanism, so nothing above this layer has
    /// to know whether the desktop has a portal, an extension or neither.
    fn hotkey_service(&self, binding: HotkeyBinding) -> Result<Box<dyn HotkeyService>>;

    /// What the hotkey mechanism of this session can do.
    ///
    /// Synchronous on purpose: the settings page asks on every render, and
    /// offering hold-to-talk where only a press arrives is worse than not
    /// offering it at all.
    fn hotkey_capabilities(&self) -> HotkeyCapabilities;
}

/// Whether Iago can dictate on this platform yet.
///
/// Linux is the only port with an implementation behind [`Platform`]; the other
/// two answer `Unsupported` to everything. The window asks this before drawing a
/// control center full of cards that cannot work.
pub fn supported() -> bool {
    cfg!(target_os = "linux")
}

/// Whether the desktop exposes the `GlobalShortcuts` portal.
///
/// Asked with `gdbus` on purpose: the settings page needs the answer while it
/// renders and the window is built without the portal client feature. The answer
/// is cached; the interfaces do not come and go.
pub fn has_global_shortcuts() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("gdbus")
            .args([
                "introspect",
                "--session",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
            ])
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .contains("org.freedesktop.portal.GlobalShortcuts")
            })
            .unwrap_or(false)
    })
}

/// Whether the session is GNOME, which is as far as any desktop guess goes.
///
/// It lives here because it is a question about the session, and only this layer
/// is allowed to answer those.
pub fn is_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
        .contains("gnome")
}

/// The platform this binary was built for.
pub fn current() -> &'static dyn Platform {
    #[cfg(target_os = "linux")]
    {
        &linux::LinuxPlatform
    }
    #[cfg(target_os = "macos")]
    {
        &macos::MacPlatform
    }
}
