// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The autostart entry of the desktop.
//!
//! The setting is the source of truth and this file follows it: the daemon writes
//! or removes an XDG autostart entry every time it starts and every time the
//! settings are reloaded. XDG autostart is the one place GNOME, KDE and the rest
//! agree on, and it needs no privileged installation.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// File name of the entry, so it can be replaced and removed.
const ENTRY: &str = "com.achetronic.Iago.desktop";

/// Makes the desktop match the setting.
pub fn apply(enabled: bool) -> Result<()> {
    let path = entry_path()?;

    if !enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "autostart entry removed"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(Error::Config(format!(
                    "cannot remove the autostart entry: {err}"
                )));
            }
        }
        return Ok(());
    }

    let executable = std::env::current_exe()
        .map_err(|err| Error::Config(format!("cannot find my own binary: {err}")))?;
    let contents = contents(&executable.display().to_string());
    let wanted = std::fs::read_to_string(&path).unwrap_or_default();
    if wanted == contents {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::Config(format!("cannot create {}: {err}", parent.display())))?;
    }
    std::fs::write(&path, contents)
        .map_err(|err| Error::Config(format!("cannot write the autostart entry: {err}")))?;
    tracing::info!(path = %path.display(), "autostart entry written");

    Ok(())
}

/// Where the entry lives: `$XDG_CONFIG_HOME/autostart`.
fn entry_path() -> Result<PathBuf> {
    let config = dirs::config_dir().ok_or_else(|| {
        Error::Config("no configuration directory, cannot place the autostart entry".to_string())
    })?;

    Ok(config.join("autostart").join(ENTRY))
}

/// The entry itself: the daemon, not the window.
fn contents(executable: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Iago\n\
         Comment=Dictation\n\
         Exec={executable} daemon run\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_starts_the_daemon() {
        let entry = contents("/usr/bin/iago");

        assert!(entry.contains("Exec=/usr/bin/iago daemon run"));
        assert!(entry.contains("Type=Application"));
        assert!(entry.contains("NoDisplay=true"));
    }
}
