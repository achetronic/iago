// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! macOS port, not written yet.
//!
//! The plan, in the order the pieces need to land:
//!
//! 1. **Audio**: `AVCaptureSession` with an `AVCaptureDevice` audio input, or
//!    `cpal` if we prefer one audio API across platforms. Either way the trait
//!    contract is the same: mono `f32` at 16 kHz through a channel.
//! 2. **Delivery**: `NSPasteboard` for the clipboard and `CGEventPost` with a
//!    `CGEventKeyboardSetUnicodeString` event for typing. Both need the
//!    Accessibility permission, granted per application bundle.
//! 3. **Hotkeys**: `CGEventTap` for a global tap, or `RegisterEventHotKey`.
//!    The latter is cleaner because it does not require Accessibility.
//! 4. **Packaging**: code signing, hardened runtime, `com.apple.security.device.audio-input`
//!    entitlement and notarisation.
//!
//! Until this file has real implementations, every capability reports
//! `Unsupported`, so the UI says so instead of failing mid-dictation.

use crate::audio::AudioBackend;
use crate::config::{AudioSettings, DeliverySettings};
use crate::error::{Error, Result};
use crate::input::TextSink;
use crate::input::hotkey::{HotkeyBinding, HotkeyCapabilities, HotkeyService};

use super::{Capabilities, Platform};

/// macOS implementation placeholder.
pub struct MacPlatform;

impl Platform for MacPlatform {
    fn id(&self) -> &'static str {
        "macos"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: "macos",
            clipboard: false,
            text_injection: "unavailable (not implemented)",
            global_hotkeys: "unavailable (not implemented)",
            audio: "unavailable (not implemented)",
        }
    }

    fn audio_backend(&self, _settings: &AudioSettings) -> Result<Box<dyn AudioBackend>> {
        Err(Error::unsupported("audio capture on macOS"))
    }

    fn text_sink(&self, _settings: &DeliverySettings) -> Result<Box<dyn TextSink>> {
        Err(Error::unsupported("text delivery on macOS"))
    }

    fn hotkey_service(&self, _binding: HotkeyBinding) -> Result<Box<dyn HotkeyService>> {
        Err(Error::unsupported("global hotkeys on macOS"))
    }

    fn hotkey_capabilities(&self) -> HotkeyCapabilities {
        // The plan is RegisterEventHotKey or a CGEventTap; neither exists yet.
        HotkeyCapabilities::PRESS_ONLY
    }
}
