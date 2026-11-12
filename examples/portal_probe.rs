// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Asks the desktop portal for a global shortcut and prints when it activates
//! and deactivates.
//!
//! The question it answers: does `Deactivated` arrive when the key is let go, or
//! the instant the shortcut fires? Hold-to-talk needs the first. Run it, press
//! the combination for a couple of seconds, and read the deltas.
//!
//! Run it with `cargo run --example portal_probe` and press the combination.
//!
//! Measured on GNOME Shell 50.1 / xdg-desktop-portal 1.21: the first bind shows a
//! dialog, takes about three seconds and stores the shortcut with the preferred
//! trigger under `org.gnome.settings-daemon.global-shortcuts`. A second bind
//! answers instantly from that store, with no dialog, because the shortcut is no
//! longer new. That also means an empty answer is what a *previously stored but
//! trigger-less* entry looks like: reset the app's dconf path before concluding
//! that the portal is broken.
//!
//! The shell grabs these shortcuts when it starts, so a freshly bound one does
//! not fire until the next login.

use std::time::Instant;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app_id: ashpd::AppID = "com.achetronic.Iago".parse()?;
    if let Err(err) = ashpd::register_host_app(app_id).await {
        eprintln!("registry refused the app id: {err}");
    }

    let proxy = GlobalShortcuts::new().await?;
    println!("portal version {}", proxy.version());

    let session = proxy.create_session(Default::default()).await?;
    let shortcut =
        NewShortcut::new("dictate", "Probe dictation").preferred_trigger("<Control><Shift>d");

    let asked = Instant::now();
    let bound = proxy
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    println!("bind answered after {:.2}s", asked.elapsed().as_secs_f32());

    for assigned in bound.shortcuts() {
        println!(
            "bound id={} trigger={:?} description={:?}",
            assigned.id(),
            assigned.trigger_description(),
            assigned.description()
        );
    }

    if bound.shortcuts().is_empty() {
        eprintln!("the portal bound nothing, asking what it has");
        match proxy.list_shortcuts(&session, Default::default()).await {
            Ok(list) => {
                let listed = list.response()?;
                for item in listed.shortcuts() {
                    println!(
                        "listed id={} trigger={:?}",
                        item.id(),
                        item.trigger_description()
                    );
                }
                println!("listed {} shortcut(s)", listed.shortcuts().len());
            }
            Err(err) => eprintln!("ListShortcuts failed: {err}"),
        }
        return Ok(());
    }

    let mut activated = Box::pin(proxy.receive_activated().await?);
    let mut deactivated = Box::pin(proxy.receive_deactivated().await?);
    let start = Instant::now();

    println!("listening: press the combination and hold it for two seconds");

    loop {
        tokio::select! {
            event = activated.next() => {
                let Some(event) = event else { break };
                println!(
                    "ACTIVATED   +{:.2}s id={}",
                    start.elapsed().as_secs_f32(),
                    event.shortcut_id()
                );
            }
            event = deactivated.next() => {
                let Some(event) = event else { break };
                println!(
                    "DEACTIVATED +{:.2}s id={}",
                    start.elapsed().as_secs_f32(),
                    event.shortcut_id()
                );
            }
        }
    }

    Ok(())
}
