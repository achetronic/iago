// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The daemon: the only process that records, transcribes and delivers.
//!
//! Keeping one owner is not bureaucracy. `iago ctl` runs as a fresh process
//! each time the shortcut fires, so the recording must live somewhere else. The
//! clipboard has the same problem: on Wayland the process that owns the
//! selection has to stay alive to serve it.
//!
//! The daemon exposes [`crate::ipc`] on a Unix socket and listens to the
//! platform hotkey at the same time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::asr::Engine;
use crate::config::{ConfigStore, HotkeyMode};
use crate::dictation::{self, Session};
use crate::error::{Error, Result};
use crate::input::TextSink;
use crate::input::hotkey::{HotkeyBinding, HotkeyEvent, Trigger};

use crate::ipc::{Action, EngineView, Phase, Request, Response, StateView};
use crate::paths::Paths;
use crate::platform;

/// Two toggles closer than this are the same intent.
const TOGGLE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

/// Below this RMS level the recording is treated as silence.
const SILENCE_THRESHOLD: f32 = 0.0015;

/// Longest socket path we accept, a `sun_path` limit with room to spare.
const MAX_SOCKET_PATH: usize = 100;

/// Mutable runtime state, guarded by a plain mutex because every critical
/// section is short and never crosses an await point.
struct Runtime {
    engine: EngineView,
    shortcut: Option<crate::ipc::ShortcutView>,
    phase: Phase,
    since: Option<Instant>,
    session: Option<Session>,
    /// Timer that stops a forgotten recording. Cancelled with the recording.
    cap: Option<tokio::task::JoinHandle<()>>,
    sink: Option<Arc<dyn TextSink>>,
    detail: Option<String>,
    last_text: Option<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            engine: EngineView::default(),
            shortcut: None,
            phase: Phase::Idle,
            since: None,
            session: None,
            cap: None,
            sink: None,
            detail: None,
            last_text: None,
        }
    }
}

/// Owns the recording state and the model process.
pub struct Daemon {
    settings: Arc<ConfigStore>,
    paths: Arc<Paths>,
    engine: Engine,
    runtime: Mutex<Runtime>,
    stopping: AtomicBool,
    last_toggle: Mutex<Option<Instant>>,
    /// Counts what the user did, so an idle engine can be recognised.
    activity: std::sync::atomic::AtomicU64,
    hotkey_runner: Mutex<Option<Box<dyn crate::input::hotkey::HotkeyRunner>>>,
    /// Combination and mode the running mechanism was built for.
    hotkey_active: Mutex<Option<(String, HotkeyMode)>>,
}

impl Daemon {
    /// Builds the daemon. Nothing is spawned until the first request.
    pub fn new(settings: Arc<ConfigStore>, paths: Arc<Paths>) -> Arc<Self> {
        let engine = Engine::from_settings(Arc::clone(&settings), Arc::clone(&paths));
        Arc::new(Self {
            settings,
            paths,
            engine,
            runtime: Mutex::new(Runtime::default()),
            stopping: AtomicBool::new(false),
            activity: std::sync::atomic::AtomicU64::new(0),
            last_toggle: Mutex::new(None),
            hotkey_runner: Mutex::new(None),
            hotkey_active: Mutex::new(None),
        })
    }

    /// Current state, safe to call from the UI at any time.
    pub fn status(&self) -> StateView {
        let runtime = self.runtime.lock().expect("runtime lock poisoned");
        StateView {
            shortcut: runtime.shortcut.clone(),
            engine: runtime.engine.clone(),
            phase: runtime.phase,
            elapsed_secs: runtime.since.map(|since| since.elapsed().as_secs()),
            detail: runtime.detail.clone(),
            last_text: runtime.last_text.clone(),
        }
    }

    /// The user pressed the shortcut.
    ///
    /// The mode decides what that means, so the sender never has to know it: a
    /// toggle-mode press starts or stops, a hold-mode press opens the microphone
    /// and waits for the release.
    pub async fn gesture_started(self: &Arc<Self>) -> Result<()> {
        match self.settings.snapshot().hotkey.mode {
            HotkeyMode::Toggle => self.toggle().await.map(|_| ()),
            HotkeyMode::PushToTalk => {
                if self.is_recording() {
                    Ok(())
                } else {
                    self.start().await
                }
            }
        }
    }

    /// The user released the shortcut.
    ///
    /// Only hold mode ends here; in toggle mode the second press does.
    pub async fn gesture_ended(self: &Arc<Self>) -> Result<Option<String>> {
        if self.settings.snapshot().hotkey.mode != HotkeyMode::PushToTalk {
            return Ok(None);
        }

        if self.is_recording() {
            self.stop().await
        } else {
            Ok(None)
        }
    }

    /// Starts or stops depending on the current phase.
    ///
    /// Ignored when another toggle arrived a moment ago: a desktop that fired the
    /// shortcut twice would otherwise start and stop in the same breath, and the
    /// user would see nothing happen.
    pub async fn toggle(self: &Arc<Self>) -> Result<Option<String>> {
        {
            let mut last = self.last_toggle.lock().expect("toggle lock poisoned");
            if last.is_some_and(|at| at.elapsed() < TOGGLE_DEBOUNCE) {
                tracing::debug!("toggle ignored, another one just arrived");
                return Ok(None);
            }
            *last = Some(Instant::now());
        }

        let recording = {
            let runtime = self.runtime.lock().expect("runtime lock poisoned");
            runtime.session.is_some()
        };

        if recording {
            self.stop().await
        } else {
            self.start().await?;
            Ok(None)
        }
    }

    /// Starts a recording.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        {
            let runtime = self.runtime.lock().expect("runtime lock poisoned");
            if runtime.session.is_some() {
                return Err(Error::Daemon("already recording".to_string()));
            }
        }

        let settings = self.settings.snapshot();
        let backend = platform::current().audio_backend(&settings.audio)?;
        let session = Session::start(backend.as_ref())?;

        // The guard goes away before anything else locks the runtime: this is a
        // plain mutex, and locking it twice on the same thread hangs the whole
        // daemon.
        {
            let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
            runtime.session = Some(session);
            runtime.phase = Phase::Recording;
            runtime.since = Some(Instant::now());
            runtime.detail = Some(format!("capturing with {}", backend.id()));
        }

        // Safety cap: a forgotten key must not record for an hour. The daemon
        // stops and transcribes on its own when the limit is reached.
        let cap = settings.hotkey.max_duration_secs;
        tracing::info!(cap_secs = cap, "recording started");

        if cap > 0 {
            let this = Arc::clone(self);
            let deadline = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(u64::from(cap))).await;
                tracing::info!("recording cap reached, stopping");
                if let Err(err) = this.stop().await {
                    tracing::warn!(error = %err, "cannot stop at the recording cap");
                }
            });

            // The timer belongs to this recording. Left running, it would stop
            // whatever is being recorded two minutes later, which the user reads
            // as the shortcut releasing itself.
            if let Some(previous) = self
                .runtime
                .lock()
                .expect("runtime lock poisoned")
                .cap
                .replace(deadline)
            {
                previous.abort();
            }
        }

        Ok(())
    }

    /// Stops the recording and answers immediately.
    ///
    /// Transcription and delivery run in a detached task. A delivery waiting on
    /// a portal that never answers used to sit inside this call for up to
    /// fourteen seconds, and the shortcut client was stuck waiting for the
    /// answer, so the next press went nowhere. The gesture that ends a recording
    /// must never be held hostage by the delivery.
    pub async fn stop(self: &Arc<Self>) -> Result<Option<String>> {
        let (session, spec) = {
            let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
            if let Some(cap) = runtime.cap.take() {
                cap.abort();
            }
            match runtime.session.take() {
                Some(session) => {
                    let spec = session.spec();
                    runtime.phase = Phase::Transcribing;
                    runtime.since = Some(Instant::now());
                    runtime.detail = Some("waiting for the model".to_string());
                    (session, spec)
                }
                None => return Err(Error::Daemon("not recording".to_string())),
            }
        };

        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.finish_and_deliver(session, spec).await;
        });

        Ok(None)
    }

    /// Captures, transcribes and delivers one finished recording. Runs detached.
    async fn finish_and_deliver(self: Arc<Self>, session: Session, spec: crate::audio::AudioSpec) {
        let samples = match tokio::task::spawn_blocking(move || session.finish()).await {
            Ok(samples) => samples,
            Err(err) => {
                let message = format!("the capture thread panicked: {err}");
                self.fail_with(message.clone());
                self.notify("Transcription failed", &message);
                return;
            }
        };

        if dictation::is_silent(&samples, SILENCE_THRESHOLD) {
            tracing::info!(samples = samples.len(), "recording was silent");
            self.finish_with(None, Some("the recording was silent".to_string()));
            return;
        }
        // Decoders lose the last phoneme when the audio stops dead. A quarter of
        // a second of silence is enough for the model to close the word, and it
        // costs nothing.
        let mut padded = samples.clone();
        padded.resize(samples.len() + spec.sample_rate as usize / 4, 0.0);

        let payload = dictation::encode(&padded, spec);
        let payload_len = payload.len();
        let language = self.settings.snapshot().asr.language;
        let language = crate::asr::optional(&language);
        let seconds = samples.len() as f32 / spec.sample_rate.max(1) as f32;
        tracing::info!(samples = samples.len(), seconds, "transcribing");

        let transcript = match self.engine.transcribe(payload, language.as_deref()).await {
            Ok(transcript) => transcript,
            Err(err) => {
                self.set_engine("failed", Some(err.to_string()));
                self.fail_with(err.to_string());
                self.notify("Transcription failed", &err.to_string());
                return;
            }
        };

        tracing::info!(
            audio_seconds = seconds,
            wav_bytes = payload_len,
            chars = transcript.text.chars().count(),
            "transcribed"
        );
        self.schedule_reaper();

        if transcript.is_empty() {
            self.finish_with(None, Some("the model returned no text".to_string()));
            return;
        }

        let text = transcript.text.clone();
        let sink = match self.sink() {
            Ok(sink) => sink,
            Err(err) => {
                self.fail_with(err.to_string());
                return;
            }
        };
        let to_deliver = text.clone();
        let duration = seconds;

        let delivery = match tokio::time::timeout(
            std::time::Duration::from_secs(12),
            tokio::task::spawn_blocking(move || sink.deliver(&to_deliver)),
        )
        .await
        {
            // The closure's result, inside the spawned task, inside the timeout.
            Ok(Ok(Ok(delivered))) => Ok(delivered),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(err)) => Err(Error::Delivery(format!(
                "the delivery thread panicked: {err}"
            ))),
            Err(_) => Err(Error::Delivery("delivery took too long".to_string())),
        };

        if let Err(err) = &delivery {
            self.fail_with(err.to_string());
        }

        match delivery {
            Ok(delivered) => {
                if delivered.method == "clipboard"
                    && self.settings.snapshot().delivery.target
                        != crate::config::DeliveryTarget::Clipboard
                {
                    self.notify(
                        "The text is in the clipboard",
                        "It could not be pasted into the window you were using.",
                    );
                }
                self.finish_with(
                    Some(text),
                    Some(format!(
                        "{} chars from {duration:.1} s via {}",
                        delivered.chars, delivered.method
                    )),
                );
            }
            Err(err) => {
                self.fail_with(err.to_string());
            }
        }
    }

    /// Makes the desktop's autostart entry match the setting.
    fn sync_autostart(&self) {
        if let Err(err) = crate::autostart::apply(self.settings.snapshot().general.autostart) {
            tracing::warn!(error = %err, "cannot update the autostart entry");
        }
    }

    /// Says something to the user without stealing the focus.
    ///
    /// Silent unless the setting is on; the notice is a courtesy either way.
    fn notify(&self, summary: &str, body: &str) {
        if self.settings.snapshot().general.notifications {
            crate::notify::send(summary, body);
        }
    }

    /// Stops the model process after the configured time without doing anything.
    ///
    /// The engine is a process with a few hundred megabytes of models in it, and
    /// leaving it up forever for a dictation every other day is a waste. Zero
    /// means never.
    fn schedule_reaper(self: &Arc<Self>) {
        let idle = self.settings.snapshot().asr.idle_timeout_secs;
        if idle == 0 {
            return;
        }

        // Every transcription bumps the counter, so a stale timer sees a different
        // number and leaves the engine alone.
        let generation = self.activity.fetch_add(1, Ordering::SeqCst) + 1;
        let this = Arc::clone(self);

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(idle)).await;

            if this.activity.load(Ordering::SeqCst) != generation {
                return;
            }
            if this.is_recording() {
                return;
            }

            let running = {
                let runtime = this.runtime.lock().expect("runtime lock poisoned");
                runtime.engine.running()
            };
            if !running {
                return;
            }

            tracing::info!(idle_secs = idle, "the engine has been idle, stopping it");
            this.engine.shutdown().await;
            this.set_engine("stopped", None);
        });
    }

    /// Drops cached platform handles and reloads settings from disk.
    ///
    /// The model process keeps running: the settings UI saves on its own, so this
    /// fires while the user types and must not interrupt a dictation.
    pub async fn reload(self: &Arc<Self>) {
        {
            let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
            runtime.sink = None;
        }

        match self.settings.reload() {
            Ok(()) => tracing::info!("settings reloaded from disk"),
            Err(err) => tracing::warn!(error = %err, "cannot reload the settings"),
        }

        self.sync_autostart();

        // A new idle time applies to the engine that is already running.
        {
            let running = {
                let runtime = self.runtime.lock().expect("runtime lock poisoned");
                runtime.engine.running()
            };
            if running {
                self.schedule_reaper();
            }
        }

        // The combination belongs to the settings, so a change has to reach the
        // mechanism: release what it had and install the new one. Comparing
        // against the store is useless, it may have reloaded on its own; what
        // matters is what the running mechanism was built for.
        let settings = self.settings.snapshot();
        let wanted = (settings.hotkey.binding.clone(), settings.hotkey.mode);
        let running = self
            .hotkey_active
            .lock()
            .expect("hotkey lock poisoned")
            .clone();

        if running.as_ref() != Some(&wanted) {
            self.spawn_hotkeys();
        }
    }

    /// Stops the model process, so the next dictation picks up new model files.
    pub async fn restart_engine(&self) {
        self.set_engine("stopped", None);
        self.engine.shutdown().await;
        tracing::info!("model process stopped");
    }

    /// Boots the model process so the server is listening, models included.
    ///
    /// This is what "download and be ready" means: the user should not have to
    /// dictate once to warm the server up.
    pub async fn start_engine(self: &Arc<Self>) -> Result<()> {
        let already = {
            let runtime = self.runtime.lock().expect("runtime lock poisoned");
            runtime.engine.running()
        };
        if already {
            // The guard is gone before this: the reaper takes the same lock.
            self.schedule_reaper();
            return Ok(());
        }

        self.set_engine("starting", None);

        match self.engine.start().await {
            Ok(address) => {
                self.set_engine("running", Some(address));
                // Started by hand: the idle timer applies to it too.
                self.schedule_reaper();
                Ok(())
            }
            Err(err) => {
                self.set_engine("failed", Some(err.to_string()));
                Err(err)
            }
        }
    }

    /// Publishes which mechanism owns the shortcut, so the UI can say it.
    /// Records which mechanism is delivering the shortcut, or none.
    fn set_shortcut(&self, mechanism: Option<&str>) {
        let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.shortcut = mechanism.map(|mechanism| crate::ipc::ShortcutView {
            mechanism: mechanism.to_string(),
        });
    }

    fn set_engine(&self, state: &str, detail: Option<String>) {
        let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.engine = EngineView {
            state: state.to_string(),
            detail,
        };
    }

    /// Stops everything owned by the daemon.
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);

        // The listening threads are blocking ones: leaving them running keeps the
        // runtime alive and the process never exits.
        self.stop_hotkeys();

        let session = {
            let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
            runtime.session.take()
        };
        if let Some(session) = session {
            let _ = tokio::task::spawn_blocking(move || session.finish()).await;
        }

        self.engine.shutdown().await;
        tracing::info!("daemon stopped");
    }

    /// Tells the desktop portal which app is asking for the paste permission.
    ///
    /// The RemoteDesktop portal only persists a grant when it can name the app,
    /// and a host process is invisible to it until registered. Without this the
    /// paste permission is asked on every connection instead of once, which is
    /// the exact wall of dialogs nobody wants.
    async fn register_input_app(&self) {
        #[cfg(all(feature = "inject", feature = "portal"))]
        {
            use ashpd::AppID;

            let Ok(app_id) = crate::APP_ID.parse::<AppID>() else {
                tracing::warn!(app_id = crate::APP_ID, "invalid application id");
                return;
            };
            if let Err(err) = ashpd::register_host_app(app_id).await {
                tracing::debug!(error = %err, "cannot register the app id for input");
            }
        }
    }

    /// Runs the hotkey listener and the control socket until shutdown.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        self.paths.ensure()?;
        self.sync_autostart();
        let socket = self.paths.socket();
        prepare_socket(&socket)?;

        let listener = UnixListener::bind(&socket).map_err(|err| {
            Error::Daemon(format!(
                "cannot listen on {}: {err}; another instance is probably running",
                socket.display()
            ))
        })?;

        tracing::info!(socket = %socket.display(), "daemon listening");
        self.register_input_app().await;
        self.spawn_hotkeys();
        self.warm_engine();
        self.warm_delivery();

        let shutdown = Arc::new(Notify::new());
        self.spawn_signal_handler(Arc::clone(&shutdown));

        loop {
            tokio::select! {
                _ = shutdown.notified() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let daemon = Arc::clone(&self);
                            let shutdown = Arc::clone(&shutdown);
                            tokio::spawn(async move {
                                daemon.serve_client(stream, shutdown).await;
                            });
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "cannot accept a control connection");
                        }
                    }
                }
            }
        }

        let _ = std::fs::remove_file(&socket);
        self.shutdown().await;
        Ok(())
    }

    /// Opens the delivery path at boot.
    ///
    /// Simulated input may need a portal permission the first time, and asking
    /// for it while a transcript is waiting is too late: the dialog would block
    /// the delivery. Warming it here means the prompt is answered, or fails, long
    /// before the first dictation.
    fn warm_delivery(self: &Arc<Self>) {
        let settings = self.settings.snapshot();

        tokio::task::spawn_blocking(move || {
            match platform::current().text_sink(&settings.delivery) {
                Ok(sink) => tracing::info!(sink = sink.id(), "delivery ready"),
                Err(err) => tracing::warn!(error = %err, "delivery is not available yet"),
            }
        });
    }

    /// Starts the model server at boot when the binary and the models are there.
    ///
    /// The user should find a listening server, not an app that boots a model the
    /// first time they dictate.
    fn warm_engine(self: &Arc<Self>) {
        let settings = self.settings.snapshot();
        let precision = settings.asr.precision;

        if !self.paths.default_binary().is_file() {
            tracing::info!("parakeet is not installed, the server stays stopped");
            return;
        }

        let root = crate::models::root(&settings, &self.paths);
        let ready = crate::models::inspect(&root, precision)
            .iter()
            .all(|model| model.installed());

        if !ready {
            tracing::info!("models are incomplete, the server stays stopped");
            return;
        }

        let daemon = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = daemon.start_engine().await {
                tracing::warn!(error = %err, "the model server did not start");
            }
        });
    }

    /// Builds and starts the platform hotkey service.
    ///
    /// Which mechanism ends up here is the platform's business: a portal, the
    /// desktop's own shortcut settings, a shell extension that talks to this
    /// socket, or nothing at all. The daemon only knows the contract.
    fn spawn_hotkeys(self: &Arc<Self>) {
        self.stop_hotkeys();

        let settings = self.settings.snapshot();
        let trigger = Trigger::parse(&settings.hotkey.binding);
        let mode = settings.hotkey.mode;

        if !trigger.is_valid() {
            tracing::warn!(binding = %settings.hotkey.binding, "the shortcut has no key");
            self.set_shortcut(None);
            return;
        }

        let binding = HotkeyBinding {
            trigger: trigger.clone(),
            mode,
        };

        let service = match platform::current().hotkey_service(binding) {
            Ok(service) => service,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "global shortcut unavailable; bind `iago ctl` in your compositor"
                );
                self.set_shortcut(None);
                return;
            }
        };

        let reports_release = service.capabilities().reports_release;
        let (events, inbox) = std::sync::mpsc::channel::<HotkeyEvent>();

        match service.spawn(events) {
            Ok(runner) => {
                *self.hotkey_runner.lock().expect("hotkey lock poisoned") = Some(runner);
            }
            Err(err) => {
                tracing::warn!(error = %err, "cannot start the hotkey listener");
                self.set_shortcut(None);
                return;
            }
        }

        tracing::info!(
            service = service.id(),
            mode = mode.label(),
            trigger = %trigger,
            "shortcut mechanism ready"
        );

        *self.hotkey_active.lock().expect("hotkey lock poisoned") = Some((trigger.spec(), mode));

        self.set_shortcut(Some(service.id()));

        self.spawn_hotkey_loop(inbox, mode, reports_release);
    }

    /// Releases the current mechanism, if any.
    ///
    /// Used before installing a new one and on shutdown: without it the portal
    /// would keep the old combination grabbed and the next one would be ignored.
    fn stop_hotkeys(&self) {
        let running = self
            .hotkey_runner
            .lock()
            .expect("hotkey lock poisoned")
            .take();

        if let Some(runner) = running {
            runner.stop();

            // The session closes at once, but the desktop ungrabs the accelerator
            // on its own round-trip. Binding the same key before that lands is
            // refused, and the shortcut then looks dead with no error anywhere.
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }

    /// Consumes hotkey events and applies them, according to the mode.
    ///
    /// Press and release mean different things in each mode, which is why the
    /// mode travels with the loop instead of being inferred from the event.
    fn spawn_hotkey_loop(
        self: &Arc<Self>,
        inbox: std::sync::mpsc::Receiver<HotkeyEvent>,
        mode: HotkeyMode,
        reports_release: bool,
    ) {
        let daemon = Arc::clone(self);

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let mut held = false;

            while let Ok(event) = inbox.recv() {
                // A mechanism that reports the release also repeats the press for
                // as long as the key is down. Only the first one is a gesture;
                // without this, holding the shortcut in toggle mode would start and
                // stop dictation several times a second.
                if reports_release {
                    match &event {
                        HotkeyEvent::Pressed => {
                            if held {
                                continue;
                            }
                            held = true;
                        }
                        HotkeyEvent::Released => held = false,
                        HotkeyEvent::Unavailable(_) => {}
                    }
                }

                let outcome = handle.block_on(async {
                    match (mode, event) {
                        // The mechanism stopped working while we were running. Stop
                        // whatever is recording, say why, and let the status show it.
                        (_, HotkeyEvent::Unavailable(reason)) => {
                            tracing::warn!(error = %reason, "shortcut stopped working");
                            let outcome = if daemon.is_recording() {
                                daemon.stop().await.map(|_| ())
                            } else {
                                Ok(())
                            };
                            daemon.set_shortcut(None);
                            outcome
                        }
                        // Hold to talk: press opens the microphone, release closes it.
                        (HotkeyMode::PushToTalk, HotkeyEvent::Pressed) => {
                            if daemon.is_recording() {
                                Ok(())
                            } else {
                                daemon.start().await
                            }
                        }
                        (HotkeyMode::PushToTalk, HotkeyEvent::Released) => {
                            if daemon.is_recording() {
                                daemon.stop().await.map(|_| ())
                            } else {
                                Ok(())
                            }
                        }
                        // Press and press again.
                        (HotkeyMode::Toggle, HotkeyEvent::Pressed) => {
                            daemon.toggle().await.map(|_| ())
                        }
                        (HotkeyMode::Toggle, HotkeyEvent::Released) => Ok(()),
                    }
                });

                if let Err(err) = outcome {
                    tracing::warn!(error = %err, "hotkey action failed");
                }
            }
        });
    }

    /// Stops the daemon on SIGINT and SIGTERM.
    ///
    /// Without SIGTERM the model process survives a `kill`, because the child is
    /// only reaped by the graceful shutdown path. Eleven orphaned servers eating
    /// twelve gigabytes is what that looks like.
    fn spawn_signal_handler(&self, shutdown: Arc<Notify>) {
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};

                let mut terminate = signal(SignalKind::terminate()).ok();
                let mut interrupt = signal(SignalKind::interrupt()).ok();

                match terminate.as_mut() {
                    Some(terminate) => {
                        tokio::select! {
                            _ = terminate.recv() => {}
                            _ = async {
                                match interrupt.as_mut() {
                                    Some(interrupt) => { interrupt.recv().await; }
                                    None => std::future::pending::<()>().await,
                                }
                            } => {}
                        }
                    }
                    None => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }

            tracing::info!("signal received, shutting down");
            shutdown.notify_waiters();
        });
    }

    /// `true` while a capture is in flight.
    pub fn is_recording(&self) -> bool {
        let runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.session.is_some()
    }

    async fn serve_client(self: &Arc<Self>, mut stream: UnixStream, shutdown: Arc<Notify>) {
        let response = match receive(&mut stream).await {
            Ok(request) => self.handle(request, Arc::clone(&shutdown)).await,
            Err(err) => Response::error(self.status(), err.to_string()),
        };

        if let Err(err) = send(&mut stream, &response).await {
            tracing::warn!(error = %err, "cannot answer the control client");
        }
    }

    async fn handle(self: &Arc<Self>, request: Request, shutdown: Arc<Notify>) -> Response {
        if request.kind != crate::ipc::KIND_COMMAND {
            // A gesture only arrives when a desktop extension or a person sends it,
            // never from the five-second status poll, so it earns a line at the
            // default level: it is the only way to see the key reaching the daemon.
            let source = request
                .meta
                .get("source")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            tracing::info!(
                kind = %request.kind,
                action = ?request.action,
                source,
                "gesture received"
            );
        }

        tracing::debug!(
            kind = %request.kind,
            action = ?request.action,
            source = request.meta.get("source").and_then(|value| value.as_str()).unwrap_or("-"),
            "request"
        );

        match request.action {
            Action::ShortcutPress => match self.gesture_started().await {
                Ok(()) => Response::ok(self.status()),
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::ShortcutRelease => match self.gesture_ended().await {
                Ok(text) => match text {
                    Some(text) => Response::done(self.status(), text),
                    None => Response::ok(self.status()),
                },
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::DaemonStatus => Response::ok(self.status()),
            Action::DaemonAlive => Response::alive(),
            Action::RecordingToggle => match self.toggle().await {
                Ok(text) => match text {
                    Some(text) => Response::done(self.status(), text),
                    None => Response::ok(self.status()),
                },
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::RecordingStart => match self.start().await {
                Ok(()) => Response::done(self.status(), "recording"),
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::RecordingStop => match self.stop().await {
                Ok(text) => match text {
                    Some(text) => Response::done(self.status(), text),
                    None => Response::ok(self.status()),
                },
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::SettingsReload => {
                self.reload().await;
                Response::done(self.status(), "settings reloaded")
            }
            Action::EngineRestart => {
                self.restart_engine().await;
                Response::done(self.status(), "engine stopped")
            }
            Action::EngineStart => match self.start_engine().await {
                Ok(()) => Response::ok(self.status()),
                Err(err) => Response::error(self.status(), err.to_string()),
            },
            Action::EngineStop => {
                self.restart_engine().await;
                Response::ok(self.status())
            }
            Action::DaemonShutdown => {
                shutdown.notify_waiters();
                Response::done(self.status(), "shutting down")
            }
        }
    }

    /// The delivery path, created once per settings change.
    fn sink(&self) -> Result<Arc<dyn TextSink>> {
        {
            let runtime = self.runtime.lock().expect("runtime lock poisoned");
            if let Some(sink) = runtime.sink.clone() {
                return Ok(sink);
            }
        }

        let settings = self.settings.snapshot();
        let sink: Arc<dyn TextSink> = Arc::from(platform::current().text_sink(&settings.delivery)?);
        let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.sink = Some(Arc::clone(&sink));
        Ok(sink)
    }

    /// Returns to idle, recording the outcome for the UI.
    fn finish_with(&self, text: Option<String>, detail: Option<String>) {
        let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.phase = Phase::Idle;
        if let Some(text) = text {
            runtime.last_text = Some(text);
        }
        runtime.detail = detail;
        runtime.since = None;
    }

    /// Marks the last operation as failed.
    fn fail_with(&self, detail: String) {
        let mut runtime = self.runtime.lock().expect("runtime lock poisoned");
        runtime.phase = Phase::Error;
        runtime.detail = Some(detail);
        runtime.since = Some(Instant::now());
    }
}

async fn receive(stream: &mut UnixStream) -> Result<Request> {
    use tokio::io::AsyncBufReadExt;

    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Err(Error::Daemon("empty request".to_string()));
    }
    Ok(serde_json::from_str(&line)?)
}

async fn send(stream: &mut UnixStream, response: &Response) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut payload = serde_json::to_string(response)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Fails early on a stale socket, a too long path or a directory we cannot make.
fn prepare_socket(socket: &PathBuf) -> Result<()> {
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(Error::Daemon(format!(
            "socket path is too long for a Unix socket: {}",
            socket.display()
        )));
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    Ok(())
}
