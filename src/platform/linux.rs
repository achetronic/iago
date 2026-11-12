// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Linux: PipeWire capture, portal hotkeys, libei delivery.

use std::sync::Arc;

use crate::audio::AudioBackend;
use crate::audio::mock::MockBackend;
use crate::audio::pw_cat::PipewireBackend;
use crate::config::{AudioBackendKind, AudioSettings, DeliverySettings, DeliveryTarget};
use crate::error::{Error, Result};
use crate::input::hotkey::{HotkeyBinding, HotkeyCapabilities, HotkeyService};
use crate::input::{Chord, Clipboard, ClipboardSink, PasteSink, TextSink};

use super::{Capabilities, Platform};

/// Linux implementation, selected by compile target.
pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn id(&self) -> &'static str {
        "linux"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: "linux",
            clipboard: cfg!(feature = "clipboard"),
            text_injection: if cfg!(feature = "inject") {
                "libei portal or wayland virtual-keyboard"
            } else {
                "unavailable (built without the inject feature)"
            },
            global_hotkeys: if cfg!(feature = "portal") {
                "GlobalShortcuts portal, or compositor binding"
            } else {
                "compositor binding (built without the portal feature)"
            },
            audio: if PipewireBackend::available() {
                "pipewire (pw-cat)"
            } else {
                "missing (install pipewire-utils)"
            },
        }
    }

    fn audio_backend(&self, settings: &AudioSettings) -> Result<Box<dyn AudioBackend>> {
        let spec = crate::audio::AudioSpec::default();

        match settings.backend {
            AudioBackendKind::Mock => Ok(Box::new(MockBackend::new(spec))),
            AudioBackendKind::Pipewire => Ok(Box::new(PipewireBackend::new(
                spec,
                crate::asr::optional(&settings.device),
            ))),
            AudioBackendKind::Auto => {
                if PipewireBackend::available() {
                    Ok(Box::new(PipewireBackend::new(
                        spec,
                        crate::asr::optional(&settings.device),
                    )))
                } else {
                    Err(Error::Audio(
                        "no capture backend available; install pipewire-utils".to_string(),
                    ))
                }
            }
        }
    }

    fn text_sink(&self, settings: &DeliverySettings) -> Result<Box<dyn TextSink>> {
        match settings.target {
            DeliveryTarget::Clipboard => Ok(Box::new(clipboard_sink(settings.after.suffix())?)),
            DeliveryTarget::Focus => Ok(Box::new(PasteSink::new(
                shared_clipboard()?,
                paste_chord()?,
                settings.after.suffix(),
            ))),
        }
    }

    fn hotkey_service(&self, binding: HotkeyBinding) -> Result<Box<dyn HotkeyService>> {
        use crate::input::hotkey::gnome::GnomeHotkeys;

        // The portal reports press and release, so it is the mechanism to prefer
        // wherever it exists, GNOME included.
        #[cfg(feature = "portal")]
        if crate::platform::has_global_shortcuts() {
            // The desktop shortcut grabs the same combination earlier in the chain
            // and swallows it, so it has to go before the portal can have it.
            if crate::input::hotkey::gnome::available() {
                let _ = crate::input::hotkey::gnome::uninstall();
            }

            return Ok(Box::new(crate::input::hotkey::portal::PortalHotkeys::new()));
        }

        // Without a portal, the desktop's own settings can hold the shortcut and
        // run `iago ctl`.
        if crate::input::hotkey::gnome::available() {
            return Ok(Box::new(GnomeHotkeys::new(binding)));
        }

        #[cfg(feature = "portal")]
        {
            Ok(Box::new(crate::input::hotkey::portal::PortalHotkeys::new()))
        }
        #[cfg(not(feature = "portal"))]
        {
            let _ = binding;
            Err(Error::Hotkey(
                "built without the portal feature; bind `iago ctl` in your compositor config"
                    .to_string(),
            ))
        }
    }

    fn hotkey_capabilities(&self) -> HotkeyCapabilities {
        use crate::input::hotkey::HotkeyCapabilities;

        if crate::platform::has_global_shortcuts() {
            return HotkeyCapabilities::PRESS_AND_RELEASE;
        }

        // What is left reports the press and nothing else: the desktop shortcut
        // here, a compositor binding elsewhere.
        HotkeyCapabilities::PRESS_ONLY
    }
}

#[cfg(feature = "clipboard")]
fn shared_clipboard() -> Result<Arc<dyn Clipboard>> {
    Ok(Arc::new(crate::input::clipboard::ArboardClipboard::new()?))
}

#[cfg(not(feature = "clipboard"))]
fn shared_clipboard() -> Result<Arc<dyn Clipboard>> {
    Err(Error::Delivery(
        "built without the clipboard feature".to_string(),
    ))
}

fn clipboard_sink(suffix: &'static str) -> Result<ClipboardSink> {
    Ok(ClipboardSink::new(shared_clipboard()?, suffix))
}

#[cfg(feature = "inject")]
fn paste_chord() -> Result<Arc<dyn Chord>> {
    use crate::input::hotkey::Trigger;

    /// What every desktop and every application understands as paste. Not a
    /// setting: the user chooses where the text goes, not how it gets pasted.
    const PASTE: &str = "CTRL+v";

    Ok(Arc::new(crate::input::inject::KeyPresser::new(
        Trigger::parse(PASTE),
    )?))
}

#[cfg(not(feature = "inject"))]
fn paste_chord() -> Result<Arc<dyn Chord>> {
    Err(Error::Delivery(
        "built without the inject feature".to_string(),
    ))
}
