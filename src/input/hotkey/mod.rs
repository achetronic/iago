// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Hotkey port.
//!
//! Every mechanism that can tell Iago "the shortcut went down" and, when it can,
//! "the shortcut came up" implements [`HotkeyService`]. Nothing above this module
//! knows which one is in use.
//!
//! - [`portal`]: the XDG `GlobalShortcuts` portal, which also reports the release.
//! - [`gnome`]: the desktop's own shortcut settings, which run `iago ctl`.
//!
//! Reading `/dev/input` directly is intentionally not implemented.

pub mod gnome;
pub mod hold;
#[cfg(feature = "portal")]
pub mod portal;
pub mod trigger;

use std::sync::mpsc::Sender;

use crate::config::HotkeyMode;
use crate::error::Result;

pub use trigger::Trigger;

/// What the shortcut did, or why it stopped working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Key combination pressed.
    Pressed,
    /// Key combination released. Only mechanisms that report it send this.
    Released,
    /// The mechanism stopped working after it started, and this is why.
    Unavailable(String),
}

/// What a mechanism can actually do here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyCapabilities {
    /// It reports the release, so hold-to-talk is possible.
    pub reports_release: bool,
}

impl HotkeyCapabilities {
    /// A mechanism that only tells when the shortcut is pressed.
    pub const PRESS_ONLY: Self = Self {
        reports_release: false,
    };

    /// A mechanism that tells press and release.
    pub const PRESS_AND_RELEASE: Self = Self {
        reports_release: true,
    };
}

/// Shortcut the user configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    /// The combination, already parsed.
    pub trigger: Trigger,
    /// Toggle or hold.
    pub mode: HotkeyMode,
}

/// A mechanism that is already running.
///
/// Stopping has to be synchronous: the portal closes its session on the way out,
/// and binding the next combination before that lands leaves the shell holding an
/// action that no longer belongs to anyone, so the new one is never delivered.
pub trait HotkeyRunner: Send {
    /// Releases what it registered and waits for its thread to finish.
    fn stop(self: Box<Self>);
}

/// Registers the shortcut and reports events.
pub trait HotkeyService: Send + Sync {
    /// Stable identifier for logs and the UI.
    fn id(&self) -> &'static str;

    /// What this mechanism can do, so the settings page can offer only what works.
    fn capabilities(&self) -> HotkeyCapabilities;

    /// Starts listening. Events are delivered on `events` from a background
    /// task, so registering never blocks the caller.
    ///
    /// A mechanism that lives outside the daemon registers nothing here: the
    /// desktop, the shell extension or the user's compositor sends the gesture
    /// over the control socket instead.
    ///
    /// The runner is what releases the registration later, when the combination
    /// changes or the daemon stops.
    fn spawn(&self, events: Sender<HotkeyEvent>) -> Result<Box<dyn HotkeyRunner>>;
}

/// Fails with a message the UI can show next to the hotkey section.
#[cfg(feature = "portal")]
pub(crate) fn hotkey_error(detail: impl std::fmt::Display) -> crate::error::Error {
    crate::error::Error::Hotkey(format!("{detail}"))
}
