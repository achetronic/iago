// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The daemon's ambassador: the work that has to live outside it.
//!
//! The reason is always the same shape: something ties a resource to the process
//! that asked for it and only lets go when that process dies. Keeping it in the
//! daemon would mean restarting Iago to release it, so it gets a process of its
//! own that the daemon can replace in place.
//!
//! Today there is one: the desktop portal shortcut client, because a grabbed
//! accelerator belongs to whoever grabbed it. More watchers can join the `join!`
//! below, each with its own startup, when something else needs the same trick.
//!
//! It takes no arguments on purpose: everything it needs comes from the settings
//! and the known directories, so the daemon only has to start it again.

use crate::config::ConfigStore;
use crate::error::{Error, Result};
use crate::input::hotkey::Trigger;
use crate::paths::Paths;

/// Runs every watcher until one of them gives up.
pub fn run() -> Result<()> {
    let paths = Paths::resolve()?;
    let settings = ConfigStore::load(paths.config_file())?;
    let snapshot = settings.snapshot();

    let trigger = Trigger::parse(&snapshot.hotkey.binding);
    if !trigger.is_valid() {
        return Err(Error::Hotkey(format!(
            "the shortcut {:?} has no key to press",
            snapshot.hotkey.binding
        )));
    }

    let socket = paths.socket();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Hotkey(format!("cannot build the worker runtime: {err}")))?;

    #[cfg(feature = "portal")]
    {
        runtime.block_on(async move {
            // One future per watcher. Adding a second one is adding it here.
            let shortcuts = crate::input::hotkey::portal::watch(&trigger, &socket);
            let (shortcuts,) = tokio::join!(shortcuts);

            shortcuts
        })
    }

    #[cfg(not(feature = "portal"))]
    {
        let _ = (trigger, socket, runtime);
        Err(Error::Hotkey(
            "built without the portal feature; there is nothing to watch".to_string(),
        ))
    }
}
