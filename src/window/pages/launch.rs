// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! What the window says where Iago cannot dictate yet.
//!
//! The binary is the same everywhere, so on macOS the control center
//! would open and every page would explain that nothing works. This says it once,
//! with the mark and the name, instead of a window full of empty cards.

use crate::window::prelude::*;

/// Height of the mark in the splash, large enough to carry the window.
const MARK_HEIGHT: f32 = 96.0;

/// Renders the full window.
pub fn render(cx: &App) -> AnyElement {
    div()
        .v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(46.0))
        .bg(cx.theme().background)
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(16.0))
                .child(
                    img(crate::window::MARK)
                        .h(px(MARK_HEIGHT))
                        .w(px(MARK_HEIGHT * 37.0 / 32.0)),
                )
                .child({
                    let mut name = div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .child("Iago");
                    name.text_style().font_size = Some(rems(3.4).into());
                    name
                }),
        )
        .child(
            div()
                .v_flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(rems(1.05))
                        .text_color(cx.theme().muted_foreground)
                        .child("Just for Linux at launch."),
                )
                .child(
                    div()
                        .text_size(rems(1.05))
                        .text_color(cx.theme().muted_foreground)
                        .child("macOS coming soon."),
                ),
        )
        .into_any_element()
}
