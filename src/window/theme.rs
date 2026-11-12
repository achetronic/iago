// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Visual style.
//!
//! Two themes, both fixed here on purpose: the user does not choose colours. The
//! only decision is light or dark, and the system already makes it (the XDG
//! portal colour scheme, which GPUI forwards as the window appearance).
//!
//! Components read their colours from the active theme, so the pages never hard
//! code one. `themes/iago-mint.json` and `themes/iago-mint-dark.json` hold the
//! whole palette in the component library's format.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, IntoElement, ParentElement as _, Styled as _, Window, div};

/// Light theme, for daylight.
const LIGHT: &str = include_str!("../../themes/iago-clay.json");
/// Dark theme, for the rest of the day.
const DARK: &str = include_str!("../../themes/iago-clay-dark.json");

const LIGHT_NAME: &str = "Iago Clay";
const DARK_NAME: &str = "Iago Clay Dark";

/// Registers both themes and picks the one the system asks for.
pub fn apply(cx: &mut App) {
    let (light, dark) = {
        let registry = ThemeRegistry::global_mut(cx);
        registry
            .load_themes_from_str(LIGHT)
            .expect("cannot load the light theme");
        registry
            .load_themes_from_str(DARK)
            .expect("cannot load the dark theme");
        (
            registry.themes().get(LIGHT_NAME).cloned(),
            registry.themes().get(DARK_NAME).cloned(),
        )
    };

    {
        let theme = Theme::global_mut(cx);
        if let Some(light) = light {
            theme.light_theme = light;
        }
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
    }

    // Start in light; the first window syncs with the system appearance.
    Theme::change(ThemeMode::Light, None, cx);
}

/// Applies the theme with the given name, switching only when it differs.
///
/// `auto` follows the desktop appearance; `mint` and `mint-dark` pin the mode.
pub fn apply_name(name: &str, window: Option<&mut Window>, cx: &mut App) {
    let wanted = match name {
        "mint" => ThemeMode::Light,
        "mint-dark" => ThemeMode::Dark,
        _ => match window.as_deref() {
            Some(window) => match window.appearance() {
                gpui_kit::WindowAppearance::Dark => ThemeMode::Dark,
                _ => ThemeMode::Light,
            },
            None => ThemeMode::Light,
        },
    };

    if cx.theme().mode != wanted {
        Theme::change(wanted, window, cx);
    }
}

/// Name the toggle should write next, given what is on screen.
pub fn toggled_name(name: &str, cx: &App) -> &'static str {
    let dark = match name {
        "mint" => false,
        "mint-dark" => true,
        _ => cx.theme().mode == ThemeMode::Dark,
    };

    if dark { "mint" } else { "mint-dark" }
}

/// Distance between main sections. UI rule: every page separates its sections by
/// exactly this, so the rhythm is the same everywhere.
pub fn section_gap() -> gpui_kit::Pixels {
    gpui_kit::px(24.0)
}

/// Page title. One size, one weight, one colour: the top of the hierarchy.
pub fn page_title(title: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xl()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().foreground)
        .child(title.to_string())
}

/// Row name, one step below a card title.
pub fn row_title(title: &str, cx: &App) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(cx.theme().foreground)
        .child(title.to_string())
}

/// Secondary line: digests, versions, counts.
pub fn meta(text: impl Into<String>, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// How a status row should look.
///
/// Three states, three colours, no more: the accent for anything working or
/// present, a warm neutral for anything off, and the danger colour for what is
/// broken. Anything that wants a fourth colour is a design smell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// Present, working or selected: the accent.
    Ok,
    /// Absent, off or not selected: a warm neutral.
    Idle,
    /// Broken: danger.
    Failed,
}

impl RowState {
    /// Colour of the dot.
    pub fn colour(self, cx: &App) -> gpui_kit::Hsla {
        match self {
            Self::Ok => cx.theme().primary,
            Self::Idle => cx.theme().muted_foreground,
            Self::Failed => cx.theme().danger,
        }
    }
}

/// Presence of an artifact: the accent when it is there, neutral when it is not.
///
/// Never green: the accent already means "good", and a second positive colour is
/// exactly what made the interface look like a traffic light.
pub fn presence(present: bool, cx: &App) -> gpui_kit::Hsla {
    if present {
        cx.theme().primary
    } else {
        cx.theme().muted_foreground
    }
}

/// One key of a shortcut, drawn as a small rounded cap.
pub fn keycap(label: &str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded(gpui_kit::px(5.0))
        .bg(cx.theme().muted)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(label.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colours(source: &str) -> std::collections::BTreeSet<String> {
        let parsed: serde_json::Value = serde_json::from_str(source).expect("valid theme json");
        parsed["themes"][0]["colors"]
            .as_object()
            .expect("colors")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn both_themes_parse_and_are_registered_under_known_names() {
        let light: serde_json::Value = serde_json::from_str(LIGHT).expect("light theme");
        let dark: serde_json::Value = serde_json::from_str(DARK).expect("dark theme");

        assert_eq!(light["themes"][0]["name"], LIGHT_NAME);
        assert_eq!(light["themes"][0]["mode"], "light");
        assert_eq!(dark["themes"][0]["name"], DARK_NAME);
        assert_eq!(dark["themes"][0]["mode"], "dark");
    }

    #[test]
    fn the_two_themes_define_the_same_tokens() {
        let light = colours(LIGHT);
        let dark = colours(DARK);

        let missing_in_dark: Vec<_> = light.difference(&dark).cloned().collect();
        let missing_in_light: Vec<_> = dark.difference(&light).cloned().collect();

        assert!(
            missing_in_dark.is_empty(),
            "tokens defined in light but not dark: {missing_in_dark:?}"
        );
        assert!(
            missing_in_light.is_empty(),
            "tokens defined in dark but not light: {missing_in_light:?}"
        );
    }
}
