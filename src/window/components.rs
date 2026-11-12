// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Reusable pieces of the window.
//!
//! Every page builds its content from these, so a rule can be changed once and
//! seen everywhere. If a page needs something new, it belongs here, not in the
//! page.

use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};

use crate::window::prelude::*;
use crate::window::theme;

/// A section: title, an optional action on the right, and its content.
pub fn card(cx: &App, title: &str, action: Option<AnyElement>) -> GroupBox {
    let mut head = div()
        .h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(theme::page_title(title, cx));

    if let Some(action) = action {
        head = head.child(action);
    }

    GroupBox::new().title(head).w_full()
}

/// One status line: a dot, a name and a short state.
pub fn status_row(state: theme::RowState, name: &str, detail: &str, cx: &App) -> AnyElement {
    div()
        .h_flex()
        .gap_3()
        .items_center()
        .child(dot(state.colour(cx)))
        .child(theme::row_title(name, cx))
        .child(theme::meta(detail, cx))
        .into_any_element()
}

/// The icon that says whether an artifact is there.
pub fn artifact_icon(present: bool, cx: &App) -> impl IntoElement {
    let (icon, colour) = if present {
        (IconName::CircleCheck, theme::presence(true, cx))
    } else {
        (IconName::TriangleAlert, theme::presence(false, cx))
    };

    div().child(Icon::new(icon).size(px(14.0)).text_color(colour))
}

/// One artifact: a check when it is there, an alert when it is not, then what it
/// is and its detail.
pub fn artifact_row(
    present: bool,
    title: &str,
    detail: impl Into<String>,
    extra: Option<String>,
    cx: &App,
) -> AnyElement {
    let mut body = div()
        .v_flex()
        .gap_1()
        .child(theme::row_title(title, cx))
        .child(theme::meta(detail, cx));

    if let Some(extra) = extra {
        body = body.child(theme::meta(extra, cx));
    }

    div()
        .h_flex()
        .gap_3()
        .items_center()
        .child(artifact_icon(present, cx))
        .child(body)
        .into_any_element()
}

/// A row with a title, a detail and a control on the right. Used for actions
/// that belong to the section, like starting or stopping the server.
pub fn action_row(
    title: &str,
    detail: impl Into<String>,
    state: theme::RowState,
    action: impl IntoElement,
    cx: &App,
) -> AnyElement {
    div()
        .h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            div()
                .h_flex()
                .gap_3()
                .items_center()
                .child(dot(state.colour(cx)))
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(theme::row_title(title, cx))
                        .child(theme::meta(detail, cx)),
                ),
        )
        .child(action)
        .into_any_element()
}

/// Progress of a download: the message, the bar and the size.
pub fn progress_bar(
    message: &str,
    stage: &str,
    fraction: f32,
    megabytes: f64,
    cx: &App,
) -> AnyElement {
    div()
        .v_flex()
        .gap_2()
        .w_full()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().primary)
                .child(SharedString::from(message.to_string())),
        )
        .child(
            div()
                .h(px(6.0))
                .w_full()
                .rounded_full()
                .bg(cx.theme().muted)
                .child(
                    div()
                        .h(px(6.0))
                        .w(gpui_kit::relative(fraction.clamp(0.0, 1.0)))
                        .rounded_full()
                        .bg(cx.theme().primary),
                ),
        )
        .child(theme::meta(
            if stage.is_empty() {
                format!("{megabytes:.0} MB")
            } else {
                format!("{stage} · {megabytes:.0} MB")
            },
            cx,
        ))
        .into_any_element()
}

/// A small round indicator.
pub fn dot(colour: gpui_kit::Hsla) -> impl IntoElement {
    div().w(px(9.0)).h(px(9.0)).rounded_full().bg(colour)
}
