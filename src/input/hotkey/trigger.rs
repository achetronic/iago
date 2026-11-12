// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! One shortcut, in every dialect it has to be written in.
//!
//! The setting is stored once, as `CTRL+SHIFT+d`. The portal wants
//! `<Control><Shift>d`, the shell extension wants the same, a compositor config
//! wants `Ctrl+Shift+d` or `SUPER+v`, and macOS will want its own.
//! Translating in each place is how three of them end up disagreeing, so the
//! translation lives here and nowhere else.

use std::fmt;

/// A key combination, parsed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    /// Control is part of the combination.
    pub control: bool,
    /// Shift is part of the combination.
    pub shift: bool,
    /// Alt is part of the combination.
    pub alt: bool,
    /// Super, Logo or Windows key is part of the combination.
    pub logo: bool,
    /// The key itself, normalized: `d`, `space`, `Return`, `F5`.
    pub key: String,
}

impl Trigger {
    /// Reads the form Iago stores, tolerating case and spacing.
    pub fn parse(spec: &str) -> Self {
        let mut trigger = Self {
            control: false,
            shift: false,
            alt: false,
            logo: false,
            key: String::new(),
        };

        for part in spec
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => trigger.control = true,
                "SHIFT" => trigger.shift = true,
                "ALT" | "MOD1" => trigger.alt = true,
                "SUPER" | "LOGO" | "WIN" | "META" => trigger.logo = true,
                _ => trigger.key = key_name(part),
            }
        }

        trigger
    }

    /// Whether there is a key to press at all.
    pub fn is_valid(&self) -> bool {
        !self.key.is_empty()
    }

    /// The form Iago stores and shows.
    pub fn spec(&self) -> String {
        let mut parts = Vec::new();

        if self.control {
            parts.push("CTRL".to_string());
        }
        if self.shift {
            parts.push("SHIFT".to_string());
        }
        if self.alt {
            parts.push("ALT".to_string());
        }
        if self.logo {
            parts.push("SUPER".to_string());
        }
        parts.push(self.key.clone());

        parts.join("+")
    }

    /// The form the portal and the shell use, e.g. `<Control><Shift>d`.
    pub fn accelerator(&self) -> String {
        let mut accelerator = String::new();

        if self.control {
            accelerator.push_str("<Control>");
        }
        if self.shift {
            accelerator.push_str("<Shift>");
        }
        if self.alt {
            accelerator.push_str("<Alt>");
        }
        if self.logo {
            accelerator.push_str("<Super>");
        }
        accelerator.push_str(&self.key);

        accelerator
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.spec())
    }
}

/// Canonical name for one key, so every backend spells it the same way.
fn key_name(token: &str) -> String {
    let lower = token.to_ascii_lowercase();

    let name = match lower.as_str() {
        "space" => "space",
        "return" | "enter" => "Return",
        "tab" => "Tab",
        "backspace" => "BackSpace",
        "insert" => "Insert",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "page_up" | "pageup" => "Page_Up",
        "page_down" | "pagedown" => "Page_Down",
        "escape" | "esc" => "Escape",
        other if other.len() == 1 => return other.to_string(),
        other if is_function_key(other) => return other.to_ascii_uppercase(),
        other => return other.to_string(),
    };

    name.to_string()
}

fn is_function_key(token: &str) -> bool {
    token
        .strip_prefix('f')
        .is_some_and(|digits| !digits.is_empty() && digits.parse::<u8>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_form_survives_a_round_trip() {
        assert_eq!(Trigger::parse("CTRL+SHIFT+d").spec(), "CTRL+SHIFT+d");
        assert_eq!(Trigger::parse("ctrl + shift + D").spec(), "CTRL+SHIFT+d");
        assert_eq!(Trigger::parse("SUPER+v").spec(), "SUPER+v");
    }

    #[test]
    fn the_portal_form_is_built_from_the_same_trigger() {
        assert_eq!(
            Trigger::parse("CTRL+SHIFT+d").accelerator(),
            "<Control><Shift>d"
        );
        assert_eq!(Trigger::parse("SUPER+v").accelerator(), "<Super>v");
        assert_eq!(Trigger::parse("ALT+F5").accelerator(), "<Alt>F5");
    }

    #[test]
    fn named_keys_are_spelled_the_way_the_toolkits_expect() {
        assert_eq!(Trigger::parse("CTRL+page_up").spec(), "CTRL+Page_Up");
        assert_eq!(Trigger::parse("CTRL+enter").spec(), "CTRL+Return");
        assert_eq!(Trigger::parse("CTRL+space").spec(), "CTRL+space");
    }

    #[test]
    fn a_combination_without_a_key_is_not_valid() {
        assert!(!Trigger::parse("CTRL+SHIFT").is_valid());
        assert!(Trigger::parse("CTRL+SHIFT+d").is_valid());
    }
}
