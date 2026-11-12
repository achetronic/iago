// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Build information, the server, the models and the way into the logs.

use crate::platform;

use crate::window::app::ControlCenter;
use crate::window::components;
use crate::window::prelude::*;
use crate::window::theme;

/// Renders the about page.
pub fn render(cx: &mut Context<ControlCenter>) -> AnyElement {
    let capabilities = platform::current().capabilities();

    let mut column = div().v_flex().gap(theme::section_gap()).w_full();

    column = column.child(
        components::card(cx, "Build", None).child(
            div()
                .v_flex()
                .gap_1()
                .child(theme::meta(format!("version: {}", crate::VERSION), cx))
                .child(theme::meta(
                    format!("platform: {}", capabilities.platform),
                    cx,
                ))
                .child(theme::meta("licence: Apache-2.0", cx)),
        ),
    );

    column = column.child(
        div().h_flex().w_full().justify_start().child(
            Button::new("open-logs")
                .label("Open logs")
                .on_click(cx.listener(|this, _, _, _| open_logs(this))),
        ),
    );

    column.into_any_element()
}

/// Opens the log file with whatever the desktop uses for text files.
fn open_logs(state: &ControlCenter) {
    let daemon = state.paths.log_file("iago");
    let ui = state.paths.log_file("iago");
    let target = if daemon.is_file() { daemon } else { ui };

    let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
}
