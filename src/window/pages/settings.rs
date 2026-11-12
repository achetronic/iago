// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Settings page, generated from `settings.toml`.
//!
//! There is no per-field code here: the descriptor decides the sections, labels,
//! controls and visibility, and this module turns each entry into a widget.

use crate::window::prelude::*;
use crate::window::schema::{self, Control, FieldDef, Schema};

/// Languages the recognition model supports, from the checkpoint card.
const LANGUAGES: [(&str, &str); 25] = [
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("it", "Italian"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mt", "Maltese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sv", "Swedish"),
    ("uk", "Ukrainian"),
];

use crate::window::app::ControlCenter;
use crate::window::components;
use crate::window::theme;

/// Renders the whole settings page.
pub fn render(state: &ControlCenter, cx: &mut Context<ControlCenter>) -> AnyElement {
    let mut column = div().v_flex().gap(theme::section_gap()).w_full();

    for section in &Schema::load().section {
        let visible: Vec<&FieldDef> = section
            .field
            .iter()
            .filter(|def| def.visible(&state.form))
            .collect();

        if visible.is_empty() {
            continue;
        }

        let mut form = Form::new().gap_4();
        for def in visible {
            form = form.child(field(def, state, cx));
        }

        // Sections are the top of the hierarchy, same size as a page title.
        let mut card = components::card(cx, &section.title, None).child(form);

        if let Some(description) = &section.description {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(description.clone()),
            );
        }

        column = column.child(card);
    }

    let warnings = state.draft().warnings();
    if !warnings.is_empty() {
        let mut card = components::card(cx, "Check these", None).child(
            div().text_xs().text_color(cx.theme().danger).child(format!(
                "{} problem(s) in the current settings",
                warnings.len()
            )),
        );

        for warning in warnings {
            card = card.child(div().text_xs().text_color(cx.theme().danger).child(warning));
        }

        column = column.child(card);
    }

    column = column.child(
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format!("config: {}", state.settings.path().display())),
    );

    column.into_any_element()
}

/// One descriptor entry, as a form row.
fn field(def: &FieldDef, state: &ControlCenter, cx: &mut Context<ControlCenter>) -> Field {
    match def.control {
        Control::Text => {
            let entity = state.inputs.get(&def.key).expect("input entity");
            Field::new()
                .label(def.label.clone())
                .child(Input::new(entity))
        }
        Control::Number => {
            let entity = state.inputs.get(&def.key).expect("input entity");
            let mut input = NumberInput::new(entity);
            if let Some(unit) = &def.unit {
                input = input.suffix(div().text_xs().child(unit.clone()));
            }
            Field::new().label(def.label.clone()).child(input)
        }
        Control::Switch => {
            let checked = schema::get_bool(&state.form, &def.key);
            let key = def.key.clone();

            Field::new().label_indent(false).child(
                Switch::new(def.key.clone())
                    .label(def.label.clone())
                    .checked(checked)
                    .on_change(cx.listener(move |this, value, _, cx| {
                        schema::set_bool(&mut this.form, &key, *value);
                        cx.notify();
                    })),
            )
        }
        Control::Slider => {
            let entity = state.sliders.get(&def.key).expect("slider entity");
            let value = schema::get_number(&state.form, &def.key);
            let unit = def.unit.clone().unwrap_or_default();

            Field::new().label(def.label.clone()).child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .child(Slider::new(entity))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{value:.0}{unit}")),
                    ),
            )
        }
        Control::Keys => {
            let binding = schema::get_text(&state.form, &def.key);
            let capturing = state.capturing;
            let caps = if capturing && !state.capture_preview.is_empty() {
                state.capture_preview.clone()
            } else {
                crate::window::app::keycaps(&binding)
            };

            let mut row = div()
                .id("binding-capture")
                .h_flex()
                .gap_1()
                .items_center()
                .px_3()
                .py_2()
                .rounded(px(7.0))
                .track_focus(&state.binding_focus)
                .bg(if capturing {
                    cx.theme().accent
                } else {
                    cx.theme().muted
                })
                .on_click(cx.listener(|this, _, window, cx| this.begin_capture(window, cx)))
                .on_key_down(
                    cx.listener(|this, event: &KeyDownEvent, _, cx| this.capture_key(event, cx)),
                );

            if caps.is_empty() {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("click and press the combination"),
                );
            } else {
                for cap in caps {
                    row = row.child(theme::keycap(&cap, cx));
                }
            }

            if capturing {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("listening… esc to cancel"),
                );
            }

            Field::new().label(def.label.clone()).child(row)
        }
        Control::Microphone => {
            let entity = state.microphones.get(&def.key).expect("select entity");
            Field::new()
                .label(def.label.clone())
                .child(Select::new(entity))
        }
        Control::Paragraph => {
            let entity = state.paragraphs.get(&def.key).expect("textarea entity");
            Field::new()
                .label(def.label.clone())
                .child(Textarea::new(entity))
        }
        Control::Language => {
            let current = schema::get_text(&state.form, &def.key);
            let automatic = current.is_empty();

            let mut body = div().v_flex().gap_2().child(
                Switch::new("language-auto")
                    .label("Detect it automatically")
                    .checked(automatic)
                    .on_change(cx.listener(|this, value, _, cx| {
                        let language = if *value {
                            String::new()
                        } else {
                            "en".to_string()
                        };
                        schema::set_text(&mut this.form, "asr.language", &language);
                        cx.notify();
                    })),
            );

            if !automatic {
                let mut chips = div().h_flex().flex_wrap().gap_1();

                for (code, name) in LANGUAGES {
                    let button = Button::new(code)
                        .label(code.to_uppercase())
                        .tooltip(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            schema::set_text(&mut this.form, "asr.language", code);
                            cx.notify();
                        }));

                    chips = chips.child(if current == code {
                        button.primary()
                    } else {
                        button
                    });
                }

                body = body.child(chips);
            }

            Field::new().label(def.label.clone()).child(body)
        }
        Control::Choice => {
            let current = schema::get_text(&state.form, &def.key);
            let mut chips = div().h_flex().flex_wrap().gap_1();

            for (index, option) in def.options.iter().enumerate() {
                let selected = current == *option;
                let available = def
                    .requirement(index)
                    .map(schema::capability)
                    .unwrap_or(true);

                let label = def.option_label(index);
                let option = option.clone();
                let key = def.key.clone();

                let button = Button::new(format!("{}-{label}", def.key))
                    .label(label)
                    .selected(selected)
                    .disabled(!available)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        schema::set_text(&mut this.form, &key, &option);
                        cx.notify();
                    }));

                chips = chips.child(if !available {
                    button.tooltip("Needs to install the extension to work")
                } else {
                    button
                });
            }

            Field::new().label(def.label.clone()).child(chips)
        }
    }
}
