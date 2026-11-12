// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The window: state, navigation and the wiring to the daemon.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::window::prelude::*;
use crate::window::schema::{self, Control, Schema};

use crate::config::{ConfigStore, Settings};
use crate::paths::Paths;

use crate::window::bridge::{self, StatusStore};
use crate::window::pages;
use crate::window::theme;

/// How long a message stays visible in the sidebar.
const MESSAGE_LIFETIME: Duration = Duration::from_secs(5);

/// The logo, vectorial and compiled in, and the height it is drawn at.
const LOGO_HEIGHT: f32 = 32.0;
/// Width that keeps the mark's own proportions (970×860, the parrot alone).
const LOGO_WIDTH: f32 = 37.0;

/// How long the form has to stay still before it is written to disk.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(600);

/// Sections of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Live state and capabilities.
    Status,
    /// Every setting, generated from the descriptor.
    Settings,
    /// The server, the models and the desktop extension.
    Dependencies,
    /// Build information and the logs.
    About,
}

impl Page {
    /// The page named by `IAGO_PAGE`, for taking screenshots of any of them.
    fn from_env() -> Option<Self> {
        let name = std::env::var("IAGO_PAGE").ok()?;
        Self::ALL
            .iter()
            .find(|(_, label)| label.eq_ignore_ascii_case(name.trim()))
            .map(|(page, _)| *page)
    }

    /// Navigation entries, in order.
    pub const ALL: [(Page, &'static str); 4] = [
        (Page::Status, "Status"),
        (Page::Settings, "Settings"),
        (Page::Dependencies, "Dependencies"),
        (Page::About, "About"),
    ];
}

/// Application state.
pub struct ControlCenter {
    /// Persisted settings.
    pub settings: Arc<ConfigStore>,
    /// Directory layout, for the models page.
    pub paths: Arc<Paths>,
    /// Control socket of the daemon.
    pub socket: PathBuf,
    /// Editable copy of the settings, shaped like the TOML file.
    pub form: Value,
    /// Last saved values, for the dirty check.
    pub saved: Settings,
    /// Current section.
    pub page: Page,
    /// Daemon snapshot.
    pub store: StatusStore,
    /// Text and number inputs, keyed by descriptor key.
    pub inputs: HashMap<String, Entity<InputState>>,
    /// Sliders, keyed by descriptor key.
    pub sliders: HashMap<String, Entity<SliderState>>,
    /// Multi-line fields, keyed by descriptor key.
    pub paragraphs: HashMap<String, Entity<TextareaState>>,
    /// Microphone dropdowns, keyed by descriptor key.
    pub microphones: HashMap<String, Entity<SelectState<crate::window::mic::SourceDelegate>>>,
    /// When the draft started differing from what is on disk, for the debounce.
    pub dirty_since: Option<Instant>,
    /// Whether the shortcut field is listening for keys.
    pub capturing: bool,
    /// Keys pressed so far, as labels for the keycaps.
    pub capture_preview: Vec<String>,
    /// Focus target of the shortcut field.
    pub binding_focus: FocusHandle,
    /// Transient message shown under the navigation.
    pub message: Option<(String, Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl ControlCenter {
    /// Builds the window state.
    pub fn new(
        settings: Arc<ConfigStore>,
        paths: Arc<Paths>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let saved = settings.snapshot();
        let models_root = if saved.asr.models_dir.is_empty() {
            paths.models_dir()
        } else {
            PathBuf::from(&saved.asr.models_dir)
        };
        let store = bridge::spawn_poller(
            paths.socket(),
            models_root,
            saved.asr.precision,
            Arc::clone(&paths),
            Arc::clone(&settings),
        );

        let mut this = Self {
            socket: paths.socket(),
            paths,
            settings,
            form: schema::to_form(&saved),
            saved,
            page: Page::from_env().unwrap_or(Page::Status),
            store,
            inputs: HashMap::new(),
            sliders: HashMap::new(),
            paragraphs: HashMap::new(),
            microphones: HashMap::new(),
            dirty_since: None,
            capturing: false,
            capture_preview: Vec::new(),
            binding_focus: cx.focus_handle(),
            message: None,
            _subscriptions: Vec::new(),
        };

        // A setting this environment cannot honour must not stay selected.
        schema::normalise(&mut this.form);
        this.rebuild_inputs(window, cx);
        let name = this.saved.theme.name.clone();
        theme::apply_name(&name, Some(window), cx);
        spawn_repaint(cx);
        this
    }

    /// The settings the draft currently describes.
    pub fn draft(&self) -> Settings {
        schema::from_form(&self.form).unwrap_or_else(|| self.saved.clone())
    }

    /// `true` when the draft differs from what is on disk.
    pub fn dirty(&self) -> bool {
        self.draft() != self.saved
    }

    /// Current message, if it has not expired.
    pub fn live_message(&self) -> Option<&str> {
        self.message.as_ref().and_then(|(message, since)| {
            if since.elapsed() < MESSAGE_LIFETIME {
                Some(message.as_str())
            } else {
                None
            }
        })
    }

    fn notify_message(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), Instant::now()));
    }

    /// Writes the draft when it has been stable for a moment.
    ///
    /// Every edit lands on disk on its own: there is no Save button, so a typo in
    /// a text field must not be written on each keystroke. The debounce keeps it
    /// to one write per pause.
    pub fn autosave(&mut self, cx: &mut Context<Self>) {
        if !self.dirty() {
            self.dirty_since = None;
            return;
        }

        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        if since.elapsed() < AUTOSAVE_DELAY {
            return;
        }

        let Some(draft) = schema::from_form(&self.form) else {
            // A half-typed value; wait for the next edit.
            return;
        };

        match self.settings.replace(draft.clone()) {
            Ok(()) => {
                self.saved = draft;
                self.dirty_since = None;
                self.message = None;
                bridge::control(self.socket.clone(), crate::ipc::Action::SettingsReload);
            }
            Err(err) => self.notify_message(format!("cannot save: {err}")),
        }
        cx.notify();
    }

    /// Creates the editable controls and rewires their subscriptions.
    ///
    /// `InputState` and `SliderState` have no public setter, so rebuilding the
    /// entities is how the form is refreshed after a revert.
    fn rebuild_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut inputs = HashMap::new();
        let mut sliders = HashMap::new();
        let mut paragraphs = HashMap::new();
        let mut microphones = HashMap::new();
        let mut subscriptions = Vec::new();
        let sources = self.store.get().sources.unwrap_or_default();

        for def in Schema::load().fields() {
            if def.hidden {
                continue;
            }

            let key = def.key.clone();
            let field = def.clone();

            match def.control {
                Control::Text | Control::Number => {
                    let value = schema::get_text(&self.form, &def.key);
                    let placeholder = def.help.clone().unwrap_or_default();
                    let entity = text_input(&value, placeholder, window, cx);
                    let control = def.control;

                    subscriptions.push(cx.subscribe_in(
                        &entity,
                        window,
                        move |this: &mut Self, state, event, _, cx| {
                            if !matches!(event, InputEvent::Change) {
                                return;
                            }
                            let text = state.read(cx).value().to_string();

                            match control {
                                Control::Number => {
                                    if let Ok(number) = text.trim().parse::<f64>() {
                                        schema::set_number(&mut this.form, &key, number, &field);
                                    }
                                }
                                _ => schema::set_text(&mut this.form, &key, &text),
                            }
                            cx.notify();
                        },
                    ));

                    inputs.insert(def.key.clone(), entity);
                }
                Control::Microphone => {
                    let current = schema::get_text(&self.form, &def.key);
                    let delegate = crate::window::mic::SourceDelegate::new(&sources);
                    let selected = delegate.position_of(&current);
                    let entity = cx.new(|cx| SelectState::new(delegate, selected, window, cx));
                    let key = def.key.clone();

                    subscriptions.push(cx.subscribe_in(
                        &entity,
                        window,
                        move |this: &mut Self,
                              _,
                              event: &SelectEvent<crate::window::mic::SourceDelegate>,
                              _,
                              cx| {
                            let SelectEvent::Confirm(Some(value)) = event else {
                                return;
                            };
                            schema::set_text(&mut this.form, &key, value);
                            cx.notify();
                        },
                    ));

                    microphones.insert(def.key.clone(), entity);
                }
                Control::Paragraph => {
                    let value = schema::get_text(&self.form, &def.key);
                    let entity = paragraph(&value, window, cx);
                    let key = def.key.clone();

                    subscriptions.push(cx.subscribe_in(
                        &entity,
                        window,
                        move |this: &mut Self, state, event, _, cx| {
                            if !matches!(event, InputEvent::Change) {
                                return;
                            }
                            let text = state.read(cx).value().to_string();
                            schema::set_text(&mut this.form, &key, &text);
                            cx.notify();
                        },
                    ));

                    paragraphs.insert(def.key.clone(), entity);
                }
                Control::Slider => {
                    let value = schema::get_number(&self.form, &def.key) as f32;
                    let min = def.min.unwrap_or(0.0) as f32;
                    let max = def.max.unwrap_or(100.0) as f32;
                    let step = def.step.unwrap_or(1.0) as f32;

                    let entity = cx.new(|_| {
                        SliderState::new()
                            .min(min)
                            .max(max)
                            .step(step)
                            .default_value(value)
                    });

                    subscriptions.push(cx.subscribe_in(
                        &entity,
                        window,
                        move |this: &mut Self, _, event: &SliderEvent, _, cx| {
                            let value = match event {
                                SliderEvent::Change(value) | SliderEvent::Release(value) => *value,
                            };
                            let number = match value {
                                SliderValue::Single(value) => value,
                                SliderValue::Range(start, _) => start,
                            };
                            schema::set_number(&mut this.form, &key, number as f64, &field);
                            cx.notify();
                        },
                    ));

                    sliders.insert(def.key.clone(), entity);
                }
                _ => {}
            }
        }

        self.inputs = inputs;
        self.sliders = sliders;
        self.paragraphs = paragraphs;
        self.microphones = microphones;
        self._subscriptions = subscriptions;
    }

    /// Starts listening for keys in the shortcut field.
    pub fn begin_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.capturing = true;
        self.capture_preview.clear();
        window.focus(&self.binding_focus, cx);
        cx.notify();
    }

    /// Handles one key press while capturing.
    pub fn capture_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.capturing || event.is_held {
            return;
        }

        let keystroke = &event.keystroke;
        let key = keystroke.key.to_lowercase();

        if key == "escape" {
            self.capturing = false;
            self.capture_preview.clear();
            cx.notify();
            return;
        }

        // A modifier on its own only updates the preview.
        if is_modifier(&key) {
            self.capture_preview = modifier_labels(&keystroke.modifiers);
            cx.notify();
            return;
        }

        let trigger = trigger_from(keystroke);
        self.capture_preview = keycaps(&trigger);
        self.capturing = false;
        schema::set_text(&mut self.form, "hotkey.binding", &trigger);
        cx.notify();
    }

    /// Light/dark switch. Writes the theme name to the config, because the
    /// colours themselves are not configurable.
    fn theme_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.draft().theme.name;
        let next = theme::toggled_name(&name, cx);
        let icon = if next == "mint" {
            IconName::Sun
        } else {
            IconName::Moon
        };

        Button::new("theme-toggle")
            .ghost()
            .icon(icon)
            .tooltip(if next == "mint" {
                "switch to the light theme"
            } else {
                "switch to the dark theme"
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let name = self.draft().theme.name;
        let next = theme::toggled_name(&name, cx);

        schema::set_text(&mut self.form, "theme.name", next);
        self.saved.theme.name = next.to_string();

        if let Err(err) = self.settings.replace(self.saved.clone()) {
            self.notify_message(format!("cannot save the theme: {err}"));
        }
        cx.notify();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let column = div()
            .v_flex()
            .w(px(190.0))
            .h_full()
            .p_3()
            .justify_between()
            .bg(cx.theme().muted);

        // Navigation and its message travel together: only two children, so the
        // mark sits at the bottom and the rows stay at the top.
        let mut nav = div().v_flex().gap_1().w_full();

        for (page, label) in Page::ALL {
            let selected = self.page == page;
            let tint = cx.theme().accent;

            // A plain row: no border, no fill unless it is current or hovered.
            let mut row = div()
                .id(label)
                .h_flex()
                .items_center()
                .w_full()
                .px_3()
                .py_2()
                .rounded(px(8.0))
                .text_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .hover(move |style| style.bg(tint.opacity(0.55)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }));

            if selected {
                row = row.bg(tint);
            }

            nav = nav.child(row);
        }

        if let Some(message) = self.live_message() {
            nav = nav.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(message.to_string()),
            );
        }

        column.child(nav).child(self.signature(cx))
    }

    /// Small mark at the bottom: the logo and the name, nothing else.
    fn signature(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .justify_center()
            .w_full()
            .gap(px(10.0))
            .child(
                img(crate::window::MARK)
                    .w(px(LOGO_WIDTH))
                    .h(px(LOGO_HEIGHT)),
            )
            .child(
                // The name is set close to the mark's own height, so the two read as
                // one piece instead of a picture with a caption.
                {
                    let mut name = div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        // The same quiet grey as a menu entry that is not selected.
                        .text_color(cx.theme().muted_foreground)
                        .child("Iago");
                    name.text_style().font_size = Some(gpui_kit::rems(1.6).into());
                    name
                },
            )
    }
}

impl Render for ControlCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = self.draft().theme.name;
        theme::apply_name(&theme_name, Some(window), cx);

        if crate::window::splash() {
            return div()
                .v_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(TitleBar::new())
                .child(pages::launch::render(cx))
                .into_any_element();
        }

        self.autosave(cx);

        let content: AnyElement = match self.page {
            Page::Status => pages::status::render(self, cx),
            Page::Settings => pages::settings::render(self, cx),
            Page::Dependencies => pages::dependencies::render(self, cx),
            Page::About => pages::about::render(cx),
        };

        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                // Nothing on the left: the name lives at the bottom of the sidebar.
                TitleBar::new().child(
                    div()
                        .h_flex()
                        .w_full()
                        .pr_2()
                        .justify_end()
                        .child(self.theme_toggle(cx)),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .id("page-scroll")
                            .v_flex()
                            .flex_1()
                            .h_full()
                            .p_5()
                            .gap_3()
                            .overflow_y_scroll()
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}

/// Creates one text input prefilled with a value.
fn text_input(
    value: &str,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<ControlCenter>,
) -> Entity<InputState> {
    let value = SharedString::from(value);
    let placeholder = SharedString::from(placeholder);
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

/// Creates a multi-line text field prefilled with a value.
fn paragraph(
    value: &str,
    window: &mut Window,
    cx: &mut Context<ControlCenter>,
) -> Entity<TextareaState> {
    let value = SharedString::from(value);
    cx.new(|cx| TextareaState::new(window, cx).default_value(value))
}

/// Repaints the window on a timer so the polled state shows up.
///
/// Faster while a download runs, so the progress bar and the animation move.
fn spawn_repaint(cx: &mut Context<ControlCenter>) {
    cx.spawn(async move |this, cx| {
        loop {
            let busy = this
                .update(cx, |this, _| this.store.get().install.running)
                .unwrap_or(false);

            cx.background_executor()
                .timer(Duration::from_millis(if busy { 250 } else { 700 }))
                .await;

            if this.update(cx, |_, cx| cx.notify()).is_err() {
                return;
            }
        }
    })
    .detach();
}

/// Modifier keys, which never finish a combination on their own.
fn is_modifier(key: &str) -> bool {
    matches!(
        key,
        "super" | "control" | "ctrl" | "alt" | "shift" | "platform" | "fn" | "function"
    )
}

/// Pretty labels for the modifiers currently held.
fn modifier_labels(modifiers: &gpui_kit::Modifiers) -> Vec<String> {
    let mut labels = Vec::new();
    if modifiers.control {
        labels.push("Ctrl".to_string());
    }
    if modifiers.alt {
        labels.push("Alt".to_string());
    }
    if modifiers.shift {
        labels.push("Shift".to_string());
    }
    if modifiers.platform {
        labels.push("Super".to_string());
    }
    labels
}

/// Builds the XDG shortcut string from a keystroke, e.g. `LOGO+shift+t`.
fn trigger_from(keystroke: &Keystroke) -> String {
    let mut parts = Vec::new();
    if keystroke.modifiers.control {
        parts.push("CTRL".to_string());
    }
    if keystroke.modifiers.alt {
        parts.push("ALT".to_string());
    }
    if keystroke.modifiers.shift {
        parts.push("SHIFT".to_string());
    }
    if keystroke.modifiers.platform {
        parts.push("LOGO".to_string());
    }
    parts.push(xdg_key(&keystroke.key));
    parts.join("+")
}

/// Maps a GPUI key name to what the shortcuts spec expects.
fn xdg_key(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "space" => "space".to_string(),
        "enter" => "Return".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "BackSpace".to_string(),
        "delete" => "Delete".to_string(),
        "insert" => "Insert".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "pageup" => "Page_Up".to_string(),
        "pagedown" => "Page_Down".to_string(),
        other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => other.to_uppercase(),
        other => other.to_string(),
    }
}

/// Splits a shortcut into pretty keycaps, e.g. `LOGO+v` becomes `Super`, `V`.
pub fn keycaps(binding: &str) -> Vec<String> {
    binding
        .split('+')
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "LOGO" | "SUPER" => "Super".to_string(),
            "CTRL" => "Ctrl".to_string(),
            "ALT" => "Alt".to_string(),
            "SHIFT" => "Shift".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            }
        })
        .collect()
}
