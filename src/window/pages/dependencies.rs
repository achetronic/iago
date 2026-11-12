// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! What Iago needs to run here: the server, the models and the desktop
//! extension, each with its state and the way to install or repair it.

use crate::models::ModelInfo;

use crate::window::app::ControlCenter;
use crate::window::bridge;
use crate::window::components;
use crate::window::prelude::*;
use crate::window::theme;

/// Renders the dependencies page.
pub fn render(state: &ControlCenter, cx: &mut Context<ControlCenter>) -> AnyElement {
    let snapshot = state.store.get();

    div()
        .v_flex()
        .gap(theme::section_gap())
        .w_full()
        .child(theme::page_title("Dependencies", cx))
        .child(daemon_card(&snapshot, cx))
        .child(models_card(&snapshot, cx))
        .into_any_element()
}

fn daemon_card(snapshot: &bridge::Snapshot, cx: &mut Context<ControlCenter>) -> AnyElement {
    let install = &snapshot.binary_install;

    let button = if install.running {
        Button::new("download-parakeet")
            .label("Downloading")
            .disabled(true)
    } else if snapshot
        .binary
        .as_ref()
        .is_some_and(|binary| binary.present)
    {
        Button::new("download-parakeet")
            .label("Download again")
            .on_click(cx.listener(|this, _, _, _| download_binary(this)))
    } else {
        Button::new("download-parakeet")
            .label("Download parakeet")
            .on_click(cx.listener(|this, _, _, _| download_binary(this)))
    };

    let mut card = components::card(cx, "Daemon", Some(button.into_any_element()));

    if install.running {
        card = card.child(components::progress_bar(
            "Downloading parakeet, this takes a moment.",
            "",
            install.fraction(),
            install.total as f64 / (1024.0 * 1024.0),
            cx,
        ));
    }

    if let Some(error) = &install.error {
        card = card.child(
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(error.clone()),
        );
    }

    card = match &snapshot.binary {
        None => card.child(div().text_xs().child("looking for the binary…")),
        Some(binary) => card.child(components::artifact_row(
            binary.present,
            "Parakeet",
            binary.label(),
            None,
            cx,
        )),
    };

    // The server itself: start it, stop it, and say whether it listens. The
    // address stays internal.
    let engine = snapshot
        .status
        .as_ref()
        .map(|status| status.engine.clone())
        .unwrap_or_default();

    let engine_state = match engine.state.as_str() {
        "running" => theme::RowState::Ok,
        "starting" => theme::RowState::Ok,
        "failed" => theme::RowState::Failed,
        _ => theme::RowState::Idle,
    };

    let ready = snapshot
        .binary
        .as_ref()
        .is_some_and(|binary| binary.present)
        && snapshot
            .models
            .as_ref()
            .is_some_and(|models| !models.is_empty() && models.iter().all(ModelInfo::installed));

    let action = if engine.running() {
        Button::new("stop-engine")
            .label("Stop")
            .on_click(cx.listener(|this, _, _, _| bridge::stop_engine(this.socket.clone())))
    } else {
        Button::new("start-engine")
            .label("Start")
            .disabled(!ready)
            .on_click(cx.listener(|this, _, _, _| bridge::start_engine(this.socket.clone())))
    };

    let detail = if engine.state == "failed" {
        engine
            .detail
            .clone()
            .unwrap_or_else(|| engine.label().to_string())
    } else {
        engine.label().to_string()
    };

    card = card.child(components::action_row(
        "Server",
        detail,
        engine_state,
        action,
        cx,
    ));

    if !ready && !engine.running() {
        card = card.child(theme::meta("download parakeet and the models first", cx));
    }

    card.into_any_element()
}

fn models_card(snapshot: &bridge::Snapshot, cx: &mut Context<ControlCenter>) -> AnyElement {
    let install = &snapshot.install;

    let button = if install.running {
        Button::new("download-models")
            .label("Downloading")
            .disabled(true)
    } else if snapshot
        .models
        .as_ref()
        .is_some_and(|models| models.iter().any(ModelInfo::installed))
    {
        Button::new("download-models")
            .label("Download again")
            .on_click(cx.listener(|this, _, _, _| download(this)))
    } else {
        Button::new("download-models")
            .label("Download models")
            .on_click(cx.listener(|this, _, _, _| download(this)))
    };

    let mut card = components::card(cx, "Models", Some(button.into_any_element()));

    if install.running {
        card = card.child(components::progress_bar(
            "Downloading models, this takes a while.",
            &install.stage,
            install.fraction(),
            install.total as f64 / (1024.0 * 1024.0),
            cx,
        ));
    }

    if let Some(error) = &install.error {
        card = card.child(
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(error.clone()),
        );
    }

    match &snapshot.models {
        None => card = card.child(div().text_xs().child("reading the models…")),
        Some(models) => {
            for info in models {
                card = card.child(model_row(info, cx));
            }
        }
    }

    card.into_any_element()
}

/// One model: a check for the state, what it does, the model and its digest.
fn model_row(info: &ModelInfo, cx: &mut Context<ControlCenter>) -> AnyElement {
    components::artifact_row(
        info.installed(),
        info.role,
        format!(
            "{} ({}) · {}",
            info.name,
            info.provider,
            info.digest_label()
        ),
        None,
        cx,
    )
}

fn download(state: &mut ControlCenter) {
    bridge::install_models(
        state.store.clone(),
        state.socket.clone(),
        std::sync::Arc::clone(&state.settings),
        std::sync::Arc::clone(&state.paths),
    );
}

fn download_binary(state: &mut ControlCenter) {
    bridge::install_binary(
        state.store.clone(),
        state.socket.clone(),
        std::sync::Arc::clone(&state.paths),
    );
}
