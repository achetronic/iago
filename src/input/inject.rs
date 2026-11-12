// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Injects the paste chord through the desktop's RemoteDesktop portal.
//!
//! `enigo`'s libei backend could open the connection on GNOME but the key press
//! itself never answered, which is why the text kept ending up in the clipboard.
//! The portal has its own key injection (`NotifyKeyboardKeysym`) that needs no
//! libei device, no keymap and no X11: one session, granted once, and each key
//! is a keysym sent straight to the compositor.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

use super::Chord;
use super::hotkey::Trigger;

/// How long a press may take before the connection counts as wedged.
const PRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// X11 keysyms for the modifiers a shortcut can carry.
const KSYM_CONTROL: i32 = 0xffe3;
const KSYM_SHIFT: i32 = 0xffe1;
const KSYM_ALT: i32 = 0xffe9;
const KSYM_SUPER: i32 = 0xffeb;

/// Presses the paste chord through the portal.
pub struct KeyPresser {
    worker: Arc<Mutex<tokio::sync::mpsc::Sender<Sender<Result<()>>>>>,
    trigger: Trigger,
}

/// Spawns the thread that owns the portal session.
///
/// The command channel is async on purpose: the owner thread runs a single-thread
/// Tokio runtime, and a blocking receive would starve the zbus connection that
/// carries the portal calls, making every key press hang.
fn spawn_worker(trigger: &Trigger) -> Result<tokio::sync::mpsc::Sender<Sender<Result<()>>>> {
    let (commands, inbox) = tokio::sync::mpsc::channel::<Sender<Result<()>>>(8);
    let owned = trigger.clone();

    std::thread::Builder::new()
        .name("iago-inject".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::warn!(error = %err, "cannot start the input runtime");
                    return;
                }
            };

            runtime.block_on(run(owned, inbox));
        })
        .map_err(|err| Error::Delivery(format!("cannot start the input thread: {err}")))?;

    Ok(commands)
}

impl KeyPresser {
    /// Spawns the session and its owner thread.
    pub fn new(trigger: Trigger) -> Result<Self> {
        if !trigger.is_valid() {
            return Err(Error::Delivery(
                "the paste chord has no key to press".to_string(),
            ));
        }

        let worker = spawn_worker(&trigger)?;

        Ok(Self {
            worker: Arc::new(Mutex::new(worker)),
            trigger,
        })
    }
}

impl Chord for KeyPresser {
    fn press(&self) -> Result<()> {
        let (reply, inbox) = mpsc::channel();
        {
            let worker = self.worker.lock().expect("worker lock poisoned");
            worker
                .blocking_send(reply)
                .map_err(|_| Error::Delivery("the input thread is gone".to_string()))?;
        }

        match inbox.recv_timeout(PRESS_TIMEOUT) {
            Ok(outcome) => outcome,
            Err(_) => {
                // A wedged session is replaced: the next press opens a new one.
                let fresh = spawn_worker(&self.trigger)?;
                *self.worker.lock().expect("worker lock poisoned") = fresh;

                Err(Error::Delivery(
                    "the input connection did not answer".to_string(),
                ))
            }
        }
    }
}

/// Owns the session and serves press requests until the daemon goes away.
async fn run(trigger: Trigger, mut inbox: tokio::sync::mpsc::Receiver<Sender<Result<()>>>) {
    let Some((proxy, session)) = open_session().await else {
        while let Some(reply) = inbox.recv().await {
            let _ = reply.send(Err(Error::Delivery(
                "the input session is not available".to_string(),
            )));
        }
        return;
    };

    while let Some(reply) = inbox.recv().await {
        let outcome = press(&proxy, &session, &trigger).await;
        let _ = reply.send(outcome);
    }

    let _ = session.close().await;
}

/// Registers the app id and opens a persisted keyboard session.
async fn open_session() -> Option<(
    ashpd::desktop::remote_desktop::RemoteDesktop,
    ashpd::desktop::Session<ashpd::desktop::remote_desktop::RemoteDesktop>,
)> {
    use ashpd::desktop::PersistMode;
    use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions};

    let app_id = crate::APP_ID.parse::<ashpd::AppID>().ok()?;
    if let Err(err) = ashpd::register_host_app(app_id).await {
        tracing::debug!(error = %err, "cannot register the app id for input");
    }

    let proxy = RemoteDesktop::new().await.ok()?;
    let session = proxy.create_session(Default::default()).await.ok()?;

    let devices = SelectDevicesOptions::default()
        .set_devices(DeviceType::KEYBOARD | DeviceType::POINTER)
        .set_persist_mode(PersistMode::ExplicitlyRevoked);

    if let Err(err) = proxy.select_devices(&session, devices).await {
        tracing::debug!(error = %err, "cannot select the keyboard");
        return None;
    }

    match proxy.start(&session, None, Default::default()).await {
        Ok(request) => match request.response() {
            Ok(_) => {
                tracing::info!("input session ready");
                Some((proxy, session))
            }
            Err(err) => {
                tracing::debug!(error = %err, "the input permission was refused");
                None
            }
        },
        Err(err) => {
            tracing::debug!(error = %err, "cannot start the input session");
            None
        }
    }
}

/// Sends the chord: modifiers down, key, modifiers up.
async fn press(
    proxy: &ashpd::desktop::remote_desktop::RemoteDesktop,
    session: &ashpd::desktop::Session<ashpd::desktop::remote_desktop::RemoteDesktop>,
    trigger: &Trigger,
) -> Result<()> {
    use ashpd::desktop::remote_desktop::KeyState;

    let key = keysym_for(&trigger.key)?;

    let mut modifiers = Vec::new();
    if trigger.control {
        modifiers.push(KSYM_CONTROL);
    }
    if trigger.shift {
        modifiers.push(KSYM_SHIFT);
    }
    if trigger.alt {
        modifiers.push(KSYM_ALT);
    }
    if trigger.logo {
        modifiers.push(KSYM_SUPER);
    }

    let send =
        |keysym, state| proxy.notify_keyboard_keysym(session, keysym, state, Default::default());

    for modifier in &modifiers {
        send(*modifier, KeyState::Pressed)
            .await
            .map_err(|err| Error::Delivery(format!("cannot hold {trigger}: {err}")))?;
    }

    let pressed = send(key, KeyState::Pressed).await;
    let released = send(key, KeyState::Released).await;

    for modifier in modifiers.iter().rev() {
        let _ = send(*modifier, KeyState::Released).await;
    }

    pressed
        .and(released)
        .map_err(|err| Error::Delivery(format!("cannot press {trigger}: {err}")))
}

/// X11 keysym for a trigger key, letters and digits as ASCII, named keys spelled
/// the way the trigger already spells them.
fn keysym_for(name: &str) -> Result<i32> {
    let mut characters = name.chars();
    if let (Some(single), None) = (characters.next(), characters.next()) {
        let code = single as u32;
        if code < 0x80 {
            return Ok(code as i32);
        }
    }

    let keysym = match name {
        "space" => 0x20,
        "Return" => 0xff0d,
        "Tab" => 0xff09,
        "Escape" => 0xff1b,
        "BackSpace" => 0xff08,
        "Delete" => 0xffff,
        "Insert" => 0xff63,
        "Home" => 0xff50,
        "End" => 0xff57,
        "Page_Up" => 0xff55,
        "Page_Down" => 0xff56,
        "Up" => 0xff52,
        "Down" => 0xff54,
        "Left" => 0xff51,
        "Right" => 0xff53,
        "F1" => 0xffbe,
        "F2" => 0xffbf,
        "F3" => 0xffc0,
        "F4" => 0xffc1,
        "F5" => 0xffc2,
        "F6" => 0xffc3,
        "F7" => 0xffc4,
        "F8" => 0xffc5,
        "F9" => 0xffc6,
        "F10" => 0xffc7,
        "F11" => 0xffc8,
        "F12" => 0xffc9,
        other => {
            return Err(Error::Delivery(format!(
                "the key {other} has no keysym to press"
            )));
        }
    };

    Ok(keysym)
}

#[cfg(test)]
mod tests {
    use super::keysym_for;

    #[test]
    fn letters_are_their_ascii_codes() {
        assert_eq!(keysym_for("v").expect("v"), 0x76);
        assert_eq!(keysym_for("d").expect("d"), 0x64);
    }

    #[test]
    fn named_keys_are_mapped() {
        assert_eq!(keysym_for("Return").expect("return"), 0xff0d);
        assert_eq!(keysym_for("space").expect("space"), 0x20);
    }

    #[test]
    fn an_invented_key_is_refused() {
        assert!(keysym_for("Hyper").is_err());
    }
}
