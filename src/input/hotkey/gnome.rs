// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Shortcuts through the desktop's own settings, for desktops whose portal
//! cannot bind one without a window.
//!
//! GNOME's `GlobalShortcuts` implementation shows a confirmation dialog, so a
//! headless daemon gets an empty answer and the shortcut never exists. The
//! desktop already has a place for custom shortcuts, and the shell runs the
//! command itself, which is exactly what `iago ctl` is for.
//!
//! The entry is written with `gsettings`, is idempotent, and lives under a path
//! that says whose it is.
//!
//! The desktop runs the command, so nothing arrives on the event channel: the
//! command reaches the daemon through the control socket, exactly like a
//! compositor binding would.

use std::process::Command;
use std::sync::mpsc::Sender;

use crate::error::{Error, Result};

use super::{HotkeyBinding, HotkeyCapabilities, HotkeyEvent, HotkeyRunner, HotkeyService, Trigger};

/// Nothing to release: the desktop runs the command itself.
struct Registered;

/// Path of the custom keybinding, under GNOME's media-keys schema.
pub const PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/iago/";

/// Schema holding the list of custom keybindings.
const LIST_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
/// Schema listing the applications that own global shortcuts.
const GLOBAL_SCHEMA: &str = "org.gnome.settings-daemon.global-shortcuts";
/// Relocatable schema holding one application's global shortcuts.
const GLOBAL_APP_SCHEMA: &str = "org.gnome.settings-daemon.global-shortcuts.application";
/// Path prefix of one application's entry in that schema.
const GLOBAL_APP_PATH: &str = "/org/gnome/settings-daemon/global-shortcuts/";
/// Schema of one custom keybinding.
const ENTRY_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";

/// Name shown in the desktop's keyboard settings.
const NAME: &str = "Iago dictation";

/// The command the desktop runs when the shortcut fires.
const COMMAND: &str = "iago ctl";

/// Whether this desktop keeps custom shortcuts in gsettings.
pub fn available() -> bool {
    Command::new("gsettings")
        .args(["list-schemas"])
        .output()
        .map(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains(LIST_SCHEMA)
        })
        .unwrap_or(false)
}

/// Installs or updates the keybinding. Safe to call on every start.
pub fn install(trigger: &Trigger, command: &str) -> Result<()> {
    let accelerator = trigger.accelerator();
    let wanted = Some(accelerator.clone());
    let previous = read(ENTRY_SCHEMA, "binding")
        .ok()
        .map(|value| value.trim().trim_matches('\'').to_string());

    // Filling the entry first is not optional: the desktop daemon reads it the
    // moment it sees a path in the list, and gives up on an incomplete one
    // without retrying.
    set(ENTRY_SCHEMA, "name", &quote(NAME))?;
    set(ENTRY_SCHEMA, "command", &quote(command))?;
    set(ENTRY_SCHEMA, "binding", &quote(&accelerator))?;

    let mut entries = list()?;
    let announced = entries.iter().any(|entry| entry == PATH);

    // Touching the list on every start makes the desktop daemon register the
    // binding once per cycle, and then one key press launches the command
    // several times. Only a new combination is worth the churn, and it needs the
    // churn: rewriting the binding of an entry already in the list leaves the
    // daemon holding nothing at all.
    if announced && previous == wanted {
        tracing::debug!("shortcut already announced with the same combination");
        return Ok(());
    }

    if announced {
        entries.retain(|entry| entry != PATH);
        write_list(&entries)?;
    }

    entries.push(PATH.to_string());
    write_list(&entries)?;

    tracing::info!(
        trigger = %trigger,
        accelerator,
        "shortcut installed in the desktop settings"
    );
    Ok(())
}

/// Writes the list of custom keybinding paths.
fn write_list(entries: &[String]) -> Result<()> {
    let value = format!(
        "[{}]",
        entries
            .iter()
            .map(|entry| format!("'{entry}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    set(LIST_SCHEMA, "custom-keybindings", &value)
}

/// Removes the keybinding from the desktop settings.
pub fn uninstall() -> Result<()> {
    let mut entries = list()?;
    if !entries.iter().any(|entry| entry == PATH) {
        return Ok(());
    }

    entries.retain(|entry| entry != PATH);
    write_list(&entries)?;
    tracing::info!("shortcut removed from the desktop settings");
    Ok(())
}

/// Whether the entry already points at this command and trigger.
pub fn is_installed(trigger: &Trigger, command: &str) -> bool {
    read(ENTRY_SCHEMA, "command")
        .map(|value| value.trim().trim_matches('\'') == command)
        .unwrap_or(false)
        && read(ENTRY_SCHEMA, "binding")
            .map(|value| value.trim().trim_matches('\'') == trigger.accelerator())
            .unwrap_or(false)
}

/// Teaches GNOME which combination Iago wants for its portal shortcut.
///
/// The portal asks the user about a shortcut once. After that, GNOME keeps its own
/// copy of the combination and ignores the `preferred_trigger` the app sends, so
/// changing the shortcut in Iago would silently do nothing. Writing Iago's choice
/// into the desktop's store before binding is what keeps Iago as the place the
/// shortcut is configured: the portal still owns the permission, the combination
/// comes from here.
pub fn store_global_shortcut(
    app_id: &str,
    id: &str,
    trigger: &Trigger,
    description: &str,
) -> Result<()> {
    let path = format!("{GLOBAL_APP_PATH}{app_id}/");
    set_with(
        &format!("{GLOBAL_APP_SCHEMA}:{path}"),
        "shortcuts",
        &global_shortcut_value(id, trigger, description),
    )?;

    // The store only shows applications it has been told about.
    let mut app_ids = unquote_list(&read(GLOBAL_SCHEMA, "applications")?);
    if !app_ids.iter().any(|entry| entry == app_id) {
        app_ids.push(app_id.to_string());
        set(GLOBAL_SCHEMA, "applications", &quote_list(&app_ids))?;
    }

    tracing::info!(app_id, id, trigger = %trigger, "shortcut stored in the desktop settings");
    Ok(())
}

/// The `shortcuts` value GNOME stores for one application.
fn global_shortcut_value(id: &str, trigger: &Trigger, description: &str) -> String {
    format!(
        "[('{id}', {{'shortcuts': <['{accelerator}']>, 'description': <'{description}'>}})]",
        accelerator = trigger.accelerator(),
        description = description.replace('\'', ""),
    )
}

/// Quotes a list the way `gsettings` expects one.
fn quote_list(entries: &[String]) -> String {
    format!(
        "[{}]",
        entries
            .iter()
            .map(|entry| format!("'{entry}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The desktop's own shortcuts, as a mechanism.
pub struct GnomeHotkeys {
    binding: HotkeyBinding,
}

impl GnomeHotkeys {
    /// One mechanism for one combination.
    pub fn new(binding: HotkeyBinding) -> Self {
        Self { binding }
    }
}

impl HotkeyService for GnomeHotkeys {
    fn id(&self) -> &'static str {
        "desktop"
    }

    fn capabilities(&self) -> HotkeyCapabilities {
        HotkeyCapabilities::PRESS_ONLY
    }

    fn spawn(&self, _events: Sender<HotkeyEvent>) -> Result<Box<dyn HotkeyRunner>> {
        install(&self.binding.trigger, COMMAND)?;
        Ok(Box::new(Registered))
    }
}

/// Reads the list of custom keybinding paths.
fn list() -> Result<Vec<String>> {
    let raw = read(LIST_SCHEMA, "custom-keybindings")?;
    Ok(unquote_list(&raw))
}

/// Extracts the quoted items of a gsettings array.
fn unquote_list(raw: &str) -> Vec<String> {
    raw.split('\'')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, item)| item.to_string())
        .collect()
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', ""))
}

fn read(schema: &str, key: &str) -> Result<String> {
    let output = Command::new("gsettings")
        .args(["get", schema_arg(schema).as_str(), key])
        .output()
        .map_err(|err| Error::Hotkey(format!("cannot run gsettings: {err}")))?;

    if !output.status.success() {
        return Err(Error::Hotkey(format!(
            "gsettings cannot read {schema} {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn set(schema: &str, key: &str, value: &str) -> Result<()> {
    set_with(&schema_arg(schema), key, value)
}

/// Writes a key, with the schema argument already built.
fn set_with(schema_arg: &str, key: &str, value: &str) -> Result<()> {
    let output = Command::new("gsettings")
        .args(["set", schema_arg, key, value])
        .output()
        .map_err(|err| Error::Hotkey(format!("cannot run gsettings: {err}")))?;

    if !output.status.success() {
        return Err(Error::Hotkey(format!(
            "gsettings cannot write {schema_arg} {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}

/// The entry schema needs the path appended, the list schema does not.
fn schema_arg(schema: &str) -> String {
    if schema == ENTRY_SCHEMA {
        format!("{schema}:{PATH}")
    } else {
        schema.to_string()
    }
}

impl HotkeyRunner for Registered {
    fn stop(self: Box<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_global_shortcut_uses_the_portal_spelling() {
        let value = global_shortcut_value(
            "dictate",
            &Trigger::parse("CTRL+SHIFT+d"),
            "Dictate with Iago",
        );

        assert_eq!(
            value,
            "[('dictate', {'shortcuts': <['<Control><Shift>d']>, 'description': <'Dictate with Iago'>})]"
        );
    }

    #[test]
    fn a_list_is_quoted_the_way_gsettings_prints_it() {
        assert_eq!(
            quote_list(&["a@b".to_string(), "c@d".to_string()]),
            "['a@b', 'c@d']"
        );
        assert_eq!(quote_list(&[]), "[]");
    }

    #[test]
    fn the_binding_list_is_parsed() {
        assert_eq!(
            unquote_list("['/a/', '/b/']"),
            vec!["/a/".to_string(), "/b/".to_string()]
        );
        assert!(unquote_list("@as []").is_empty());
    }
}
