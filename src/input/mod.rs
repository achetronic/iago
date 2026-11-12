// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Text delivery and hotkey ports.
//!
//! Two independent capabilities, deliberately separated:
//!
//! - [`TextSink`] puts a finished transcript where the user wants it.
//! - [`HotkeyService`] tells the app when to start and stop recording.
//!
//! Wayland gives neither for free, so both have the same shape: a trait with a
//! Linux implementation today and a macOS one later, chosen by
//! [`crate::platform`].

#[cfg(feature = "clipboard")]
pub mod clipboard;
pub mod hotkey;
#[cfg(feature = "inject")]
pub mod inject;

use std::sync::Arc;
use std::time::Duration;

use crate::error::Result;

/// How long to wait between writing the clipboard and pressing the paste, so the
/// focused app sees the new selection.
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(250);

/// Result of a successful delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// Mechanism that actually moved the text: `clipboard`, `paste`.
    pub method: &'static str,
    /// Characters delivered.
    pub chars: usize,
}

/// One key combination, pressed into whatever has the focus.
///
/// The sink that hands the transcript over does not know which keys those are:
/// they come from the settings, and a platform without synthetic input can say
/// so at construction time.
pub trait Chord: Send + Sync {
    /// Presses the combination, blocking until the desktop took it.
    fn press(&self) -> Result<()>;
}

/// Anything that can hand text to the user.
pub trait TextSink: Send + Sync {
    /// Stable identifier for logs and the UI.
    fn id(&self) -> &'static str;

    /// Delivers `text`. Blocking: call it off the async runtime.
    fn deliver(&self, text: &str) -> Result<Delivery>;
}

/// Read/write access to the system clipboard.
pub trait Clipboard: Send + Sync {
    /// Current text content, if any.
    fn read(&self) -> Result<Option<String>>;

    /// Replaces the text content.
    fn write(&self, text: &str) -> Result<()>;
}

/// Puts the transcript in the clipboard and pastes it into the focused window.
///
/// Pasting is the way to deliver text on Wayland: synthetic input is keycode
/// based, so the transcript itself can only be typed if every character happens
/// to sit on a key of the current layout. See [`inject`] for the details.
///
/// The transcript stays in the clipboard either way. Restoring whatever the user
/// had copied looked polite, but when the paste did not land the user pasted by
/// hand and found the old clipboard content instead of their own words, one
/// phrase behind. Their transcript matters more than whatever was copied.
pub struct PasteSink {
    clipboard: Arc<dyn Clipboard>,
    chord: Arc<dyn Chord>,
    suffix: &'static str,
}

impl PasteSink {
    /// Wires the clipboard and the paste chord together.
    pub fn new(clipboard: Arc<dyn Clipboard>, chord: Arc<dyn Chord>, suffix: &'static str) -> Self {
        Self {
            clipboard,
            chord,
            suffix,
        }
    }
}

impl TextSink for PasteSink {
    fn id(&self) -> &'static str {
        "clipboard+paste"
    }

    fn deliver(&self, text: &str) -> Result<Delivery> {
        let payload = format!("{text}{}", self.suffix);

        self.clipboard.write(&payload)?;
        std::thread::sleep(CLIPBOARD_SETTLE);

        // Pasting can be unavailable (no permission, no protocol). The text is
        // already in the clipboard, so that is a delivery, not a failure: losing
        // the transcript because the paste did not work would be worse.
        let method = match self.chord.press() {
            Ok(()) => "paste",
            Err(err) => {
                tracing::warn!(error = %err, "cannot paste the transcript, left in the clipboard");
                "clipboard"
            }
        };

        Ok(Delivery {
            method,
            chars: payload.chars().count(),
        })
    }
}

/// Wraps a clipboard-only sink as a [`TextSink`].
pub struct ClipboardSink {
    clipboard: Arc<dyn Clipboard>,
    suffix: &'static str,
}

impl ClipboardSink {
    /// Creates the sink.
    pub fn new(clipboard: Arc<dyn Clipboard>, suffix: &'static str) -> Self {
        Self { clipboard, suffix }
    }
}

impl TextSink for ClipboardSink {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn deliver(&self, text: &str) -> Result<Delivery> {
        let payload = format!("{text}{}", self.suffix);
        self.clipboard.write(&payload)?;

        Ok(Delivery {
            method: "clipboard",
            chars: payload.chars().count(),
        })
    }
}
