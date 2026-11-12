// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Is the engine there, are the models there, is it dictating right now.

use crate::config::AsrMode;
use crate::ipc::Phase;

use crate::window::app::ControlCenter;
use crate::window::components;
use crate::window::prelude::*;
use crate::window::theme;

/// Renders the status page.
pub fn render(state: &ControlCenter, cx: &mut Context<ControlCenter>) -> AnyElement {
    let snapshot = state.store.get();
    let settings = state.draft();

    // UI rule: main sections are separated by `theme::SECTION_GAP`, the same
    // distance Settings uses between its cards.
    let mut column = div()
        .v_flex()
        .gap(theme::section_gap())
        .w_full()
        .child(theme::page_title("Status", cx));

    // Engine: the server we start, or the remote one when that mode is selected.
    let engine = snapshot
        .status
        .as_ref()
        .map(|status| status.engine.clone())
        .unwrap_or_default();

    let (engine_state, engine_label, engine_detail) = if settings.asr.mode == AsrMode::Remote {
        let reachable = snapshot.remote_ok.unwrap_or(false);
        (
            if reachable {
                theme::RowState::Ok
            } else {
                theme::RowState::Failed
            },
            "Remote server",
            if reachable {
                "reachable".to_string()
            } else {
                "unreachable".to_string()
            },
        )
    } else if !snapshot.online {
        (theme::RowState::Failed, "Daemon", "not running".to_string())
    } else {
        let state = match engine.state.as_str() {
            "running" => theme::RowState::Ok,
            "starting" => theme::RowState::Ok,
            "failed" => theme::RowState::Failed,
            _ => theme::RowState::Idle,
        };

        // The address stays internal: the row only says whether it listens.
        (state, "Server", engine.label().to_string())
    };

    let mut summary = div().v_flex().gap_3().w_full();
    summary = summary.child(components::status_row(
        engine_state,
        engine_label,
        &engine_detail,
        cx,
    ));

    // Models state, at a glance. The card below is for downloading them.
    let models = snapshot.models.clone().unwrap_or_default();
    let installed = models.iter().filter(|model| model.installed()).count();
    let total = models.len();
    let models_ok = total > 0 && installed == total;

    let models_detail = if total == 0 {
        "reading the model files…".to_string()
    } else {
        format!("{installed} of {total} installed")
    };

    summary = summary.child(components::status_row(
        if models_ok {
            theme::RowState::Ok
        } else {
            theme::RowState::Idle
        },
        "Models",
        &models_detail,
        cx,
    ));

    // Dictating, the state the user actually cares about.
    let recording = snapshot
        .status
        .as_ref()
        .map(|status| status.phase == Phase::Recording)
        .unwrap_or(false);
    let elapsed = snapshot
        .status
        .as_ref()
        .and_then(|status| status.elapsed_secs);

    let detail = if recording {
        match elapsed {
            Some(secs) => format!("listening · {secs}s"),
            None => "listening".to_string(),
        }
    } else {
        "not dictating".to_string()
    };

    summary = summary.child(components::status_row(
        if recording {
            theme::RowState::Ok
        } else {
            theme::RowState::Idle
        },
        "Dictating",
        &detail,
        cx,
    ));

    column = column.child(summary);

    column.into_any_element()
}
