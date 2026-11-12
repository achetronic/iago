// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Global shortcuts through the XDG portal.
//!
//! `org.freedesktop.portal.GlobalShortcuts` emits `Activated` and `Deactivated`
//! for the same shortcut, which is exactly the pair push-to-talk needs: start
//! recording on the first, stop and transcribe on the second. The portal stores
//! the grant per application, so the confirmation dialog shows up once.
//!
//! Backend support: GNOME, KDE and Hyprland. Compositors without it fall back
//! to a compositor config binding (see [`super::compositor`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

use crate::error::Result;
use crate::ipc::{Action, KIND_PUSH_TO_TALK, Request};

use super::hold::{Hold, Press};
use super::{HotkeyCapabilities, HotkeyEvent, HotkeyRunner, HotkeyService, Trigger, hotkey_error};

/// Identifier the portal stores for this shortcut.
const SHORTCUT_ID: &str = "dictate";

/// Global shortcuts driven by the desktop portal.
///
/// The combination is not carried here: the client is the ambassador process, which reads
/// the settings itself, which is what lets the daemon replace it without passing
/// anything around.
pub struct PortalHotkeys;

impl Default for PortalHotkeys {
    fn default() -> Self {
        Self
    }
}

impl PortalHotkeys {
    /// The mechanism itself.
    pub const fn new() -> Self {
        Self
    }

    /// Whether the portal interface exists on the current session bus.
    pub async fn available() -> bool {
        match GlobalShortcuts::new().await {
            Ok(proxy) => proxy.version() >= 1,
            Err(_) => false,
        }
    }
}

impl HotkeyService for PortalHotkeys {
    fn id(&self) -> &'static str {
        "portal"
    }

    fn capabilities(&self) -> HotkeyCapabilities {
        // The portal reports activation and deactivation for the same shortcut;
        // that pair is the whole reason this mechanism exists.
        HotkeyCapabilities::PRESS_AND_RELEASE
    }

    fn spawn(&self, _events: Sender<HotkeyEvent>) -> Result<Box<dyn HotkeyRunner>> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("iago-hotkeys".to_string())
            .spawn(move || supervise(thread_stop))
            .map_err(|err| hotkey_error(format!("cannot start the hotkey thread: {err}")))?;

        Ok(Box::new(PortalRunner {
            stop,
            handle: Some(handle),
        }))
    }
}

/// The supervisor thread, and the switch that ends it.
struct PortalRunner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HotkeyRunner for PortalRunner {
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::Relaxed);

        // Waiting here is the point: the child owns the grabbed accelerator, and
        // the desktop only lets go when that process is gone.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Keeps one portal client process alive, replacing it when it dies.
fn supervise(stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(executable) = std::env::current_exe() else {
            tracing::error!("cannot find this binary; the portal client will not run");
            return;
        };

        let child = std::process::Command::new(executable)
            .args(["daemon", "ambassador"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(error = %err, "cannot start the portal client, retrying");
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        };

        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::warn!(%status, "portal client exited, restarting");
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                Err(err) => {
                    tracing::warn!(error = %err, "cannot watch the portal client");
                    let _ = child.kill();
                    break;
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// Watches the shortcut and forwards it as gestures, until the caller stops it.
///
/// It runs in the ambassador process, not in the daemon: the desktop ties a grabbed
/// accelerator to the client that asked for it and only releases it when that
/// client is gone. That is what lets the daemon change the combination without
/// restarting.
pub async fn watch(trigger: &Trigger, socket: &std::path::Path) -> Result<()> {
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), register_app_id()).await;

    if crate::platform::is_gnome() {
        if let Err(err) = super::gnome::store_global_shortcut(
            crate::APP_ID,
            SHORTCUT_ID,
            trigger,
            "Dictate with Iago",
        ) {
            tracing::debug!(error = %err, "cannot store the shortcut in the desktop settings");
        }
    }

    let proxy = GlobalShortcuts::new()
        .await
        .map_err(|err| hotkey_error(format!("the GlobalShortcuts portal is unavailable: {err}")))?;

    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|err| hotkey_error(format!("cannot create a shortcuts session: {err}")))?;

    let outcome = bind_and_forward(&proxy, &session, trigger, socket).await;
    let _ = session.close().await;

    outcome
}

/// Binds the combination and turns its events into gestures.
async fn bind_and_forward(
    proxy: &GlobalShortcuts,
    session: &ashpd::desktop::Session<GlobalShortcuts>,
    trigger: &Trigger,
    socket: &std::path::Path,
) -> Result<()> {
    let accelerator = trigger.accelerator();
    let shortcut =
        NewShortcut::new(SHORTCUT_ID, "Dictate with Iago").preferred_trigger(accelerator.as_str());

    let bound = proxy
        .bind_shortcuts(session, &[shortcut], None, Default::default())
        .await
        .map_err(|err| hotkey_error(format!("cannot bind {trigger}: {err}")))?
        .response()
        .map_err(|err| hotkey_error(format!("the portal refused the shortcut: {err}")))?;

    if bound.shortcuts().is_empty() {
        return Err(hotkey_error(format!(
            "the portal bound no shortcut for {trigger}; check the trigger syntax"
        )));
    }

    for assigned in bound.shortcuts() {
        tracing::info!(
            requested = %trigger,
            id = assigned.id(),
            assigned = assigned.trigger_description(),
            "global shortcut registered"
        );
    }

    let mut activated = Box::pin(
        proxy
            .receive_activated()
            .await
            .map_err(|err| hotkey_error(format!("cannot listen for presses: {err}")))?,
    );
    let mut deactivated = Box::pin(
        proxy
            .receive_deactivated()
            .await
            .map_err(|err| hotkey_error(format!("cannot listen for releases: {err}")))?,
    );

    // A client whose daemon is gone only holds the shortcut hostage: it watches and
    // leaves. The same tick watches the portal itself: the session lives inside
    // its process, so a portal that restarts leaves us bound to nothing and no
    // event ever arrives again.
    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(5));

    // The backend that really holds the grab (gnome, kde, hyprland) is another
    // process. When it restarts, our session keeps existing in the frontend and
    // no event arrives ever again, which is indistinguishable from a shortcut
    // nobody presses. Watching its bus name is the only notice we get.
    let mut portal_changes = portal_changes().await;

    // The portal reports one `Activated` per key repeat, thirty-three a second,
    // for a key the user is holding. Forwarding each one floods the daemon with
    // hundreds of requests per hold, and a release queued behind them arrives
    // seconds late, or after a press that should not have been there. Only the
    // edge matters: down once, up once.
    let mut hold = Hold::new();

    // The release the portal owes us does not always come, so the silence after
    // the repeats is watched too. See [`Hold`].
    let mut silence = tokio::time::interval(std::time::Duration::from_millis(80));

    loop {
        tokio::select! {
            biased;

            // Presses first, deliberately: when both are already waiting, the
            // press belongs to this gesture and the release that follows ends it.
            // The other way round the release would be dropped as a stray one.
            event = activated.next() => {
                if event.is_none() {
                    return Err(hotkey_error("the portal closed the shortcuts session"));
                }
                match hold.press(std::time::Instant::now()) {
                    Press::Repeat => continue,
                    Press::Fresh => gesture(socket, Action::ShortcutPress).await?,
                }
            }
            event = deactivated.next() => {
                if event.is_none() {
                    return Err(hotkey_error("the portal closed the shortcuts session"));
                }
                if hold.release(std::time::Instant::now()) {
                    gesture(socket, Action::ShortcutRelease).await?;
                }
            }
            change = next_change(&mut portal_changes) => {
                if change.starts_with("org.freedesktop.impl.portal") {
                    return Err(hotkey_error(
                        "the portal backend restarted, binding again",
                    ));
                }
            }
            _ = silence.tick() => {
                if hold.expired(std::time::Instant::now()) {
                    tracing::debug!("the shortcut went quiet, taking it as released");
                    gesture(socket, Action::ShortcutRelease).await?;
                }
            }
            _ = watchdog.tick() => {
                if crate::ipc::call(socket, &Request::new(Action::DaemonAlive)).await.is_err() {
                    return Err(hotkey_error("the daemon is gone"));
                }

            }
        }
    }
}

/// Stream of bus names coming and going, or `None` when the bus cannot be watched.
async fn portal_changes() -> Option<ashpd::zbus::fdo::NameOwnerChangedStream> {
    use ashpd::zbus::Connection;
    use ashpd::zbus::fdo::DBusProxy;

    let connection = Connection::session().await.ok()?;
    let proxy = DBusProxy::new(&connection).await.ok()?;

    proxy.receive_name_owner_changed().await.ok()
}

/// One name change, or a future that never resolves when there is nothing to watch.
async fn next_change(stream: &mut Option<ashpd::zbus::fdo::NameOwnerChangedStream>) -> String {
    use futures_util::StreamExt;

    match stream {
        Some(stream) => match stream.next().await {
            Some(change) => match change.args() {
                Ok(args) => args.name().to_string(),
                Err(_) => std::future::pending().await,
            },
            _ => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// Tells the daemon the key went down or up.
async fn gesture(socket: &std::path::Path, action: Action) -> Result<()> {
    let request = Request::gesture(KIND_PUSH_TO_TALK, action, "portal");

    crate::ipc::call(socket, &request)
        .await
        .map(|_| ())
        .map_err(|err| hotkey_error(format!("cannot reach the daemon: {err}")))
}

/// Tells the portal who we are.
///
/// xdg-desktop-portal resolves the app id of a host process by matching its
/// executable against the installed `.desktop` files. That works once the
/// desktop entry is installed, and fails on a fresh checkout, so we also
/// register explicitly. Registration is idempotent and cheap.
async fn register_app_id() {
    use ashpd::AppID;

    let Ok(app_id) = crate::APP_ID.parse::<AppID>() else {
        tracing::warn!(app_id = crate::APP_ID, "invalid application id");
        return;
    };

    if let Err(err) = ashpd::register_host_app(app_id).await {
        tracing::debug!(error = %err, "host app registration failed");
    }
}
