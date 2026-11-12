// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Desktop notifications.
//!
//! Iago is a window you keep in the background, so when something goes wrong the
//! user is usually looking at another application. A notification is the only way
//! to say it without stealing the focus.
//!
//! It goes through `notify-send`, which is the one client every desktop ships and
//! which speaks the freedesktop notification protocol. Nothing is kept open, and a
//! machine without it simply loses the notification.

use std::process::{Command, Stdio};

/// Shows a notification. Never fails the caller: it is a courtesy, not a step.
pub fn send(summary: &str, body: &str) {
    let child = Command::new("notify-send")
        .arg("-a")
        .arg("Iago")
        .arg(summary)
        .arg(body)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match child {
        Ok(_) => tracing::debug!(summary, "notification sent"),
        Err(err) => tracing::debug!(error = %err, "no notification daemon, dropping the notice"),
    }
}
