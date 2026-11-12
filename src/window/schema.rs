// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Settings descriptor.
//!
//! The settings UI is generated from `settings.toml`, not written by hand. The
//! Rust types in [`crate::config`] stay the source of truth for values, types and
//! defaults; the descriptor decides how each one is presented.
//!
//! The two cannot drift silently: the tests at the bottom of this module check
//! that every key exists, that the control matches the field type, that every
//! choice can be deserialised into the typed settings, and that no setting is
//! left without an entry.

use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Settings;

/// The descriptor, embedded so a broken file cannot ship.
pub const DESCRIPTOR: &str = include_str!("settings.toml");

/// Whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    /// Sections, in presentation order.
    pub section: Vec<Section>,
}

/// One card of the settings page.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    /// Card title.
    pub title: String,
    /// Optional line under the title.
    #[serde(default)]
    pub description: Option<String>,
    /// Fields of the card, in order.
    pub field: Vec<FieldDef>,
}

/// One editable option.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    /// Dotted path into the settings, for example `asr.precision`.
    pub key: String,
    /// Label shown next to the control.
    pub label: String,
    /// Which control to render.
    pub control: Control,
    /// Extra explanation, shown as a tooltip or a second line.
    #[serde(default)]
    pub help: Option<String>,
    /// Unit appended to number inputs.
    #[serde(default)]
    pub unit: Option<String>,
    /// Lower bound for numbers.
    #[serde(default)]
    pub min: Option<f64>,
    /// Upper bound for numbers.
    #[serde(default)]
    pub max: Option<f64>,
    /// Granularity of a slider.
    #[serde(default)]
    pub step: Option<f64>,
    /// Serialised values of a choice, in order.
    #[serde(default)]
    pub options: Vec<String>,
    /// Labels of a choice, parallel to `options`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Capability each option needs, parallel to `options`. Empty means always
    /// available. An option whose capability is missing is shown disabled.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Only render when this condition holds, for example `asr.mode == remote`.
    #[serde(default)]
    pub visible_when: Option<String>,
    /// Setting exists but is not editable here.
    #[serde(default)]
    pub hidden: bool,
}

/// Which widget to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Control {
    /// Free text.
    Text,
    /// Numeric text with unit and bounds.
    Number,
    /// Boolean toggle.
    Switch,
    /// Radio group over `options`.
    Choice,
    /// Slider over `min..max`.
    Slider,
    /// Keyboard shortcut capture.
    Keys,
    /// Language, with an automatic switch and a code picker.
    Language,
    /// One of the capture sources of the system.
    Microphone,
    /// Multi-line text, one entry per line.
    Paragraph,
}

impl Schema {
    /// The descriptor compiled with the binary.
    pub fn load() -> &'static Schema {
        static SCHEMA: OnceLock<Schema> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            toml::from_str(DESCRIPTOR).expect("settings.toml does not match the schema")
        })
    }

    /// Every field, including hidden ones.
    pub fn fields(&self) -> impl Iterator<Item = &FieldDef> {
        self.section.iter().flat_map(|section| section.field.iter())
    }

    /// Looks a field up by key. Used by the drift tests today, and by anything
    /// that needs the descriptor entry for a known setting.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn field(&self, key: &str) -> Option<&FieldDef> {
        self.fields().find(|field| field.key == key)
    }
}

impl FieldDef {
    /// Whether this field should be rendered for the given values.
    pub fn visible(&self, form: &Value) -> bool {
        match &self.visible_when {
            None => !self.hidden,
            Some(condition) => !self.hidden && condition_holds(condition, form),
        }
    }

    /// Capability the option at `index` needs, if any.
    pub fn requirement(&self, index: usize) -> Option<&str> {
        self.requires
            .get(index)
            .map(String::as_str)
            .filter(|requirement| !requirement.is_empty())
    }

    /// Label of a choice option at `index`.
    pub fn option_label(&self, index: usize) -> String {
        self.labels
            .get(index)
            .cloned()
            .or_else(|| self.options.get(index).cloned())
            .unwrap_or_default()
    }
}

/// Editable copy of the settings, with the same shape as the TOML file.
pub fn to_form(settings: &Settings) -> Value {
    serde_json::to_value(settings).unwrap_or_else(|_| json!({}))
}

/// Typed settings out of an edited form, or `None` when the form is invalid.
pub fn from_form(form: &Value) -> Option<Settings> {
    serde_json::from_value(form.clone()).ok()
}

/// Value at a dotted path.
pub fn get<'a>(form: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(form, |value, segment| value.get(segment))
}

/// Value at a dotted path, as text.
pub fn get_text(form: &Value, key: &str) -> String {
    match get(form, key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

/// Value at a dotted path, as number.
pub fn get_number(form: &Value, key: &str) -> f64 {
    get(form, key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Value at a dotted path, as boolean.
pub fn get_bool(form: &Value, key: &str) -> bool {
    get(form, key).and_then(Value::as_bool).unwrap_or(false)
}

/// Whether this environment can do what a capability name asks for.
///
/// The answer comes from the hotkey mechanism the platform picked, not from this
/// crate: holding a key needs something that reports its release, and only the
/// mechanism knows whether it does.
pub fn capability(requirement: &str) -> bool {
    match requirement {
        "hold-to-talk" => reports_release(),
        _ => true,
    }
}

/// Whether the shortcut mechanism of this session can see a key come up.
fn reports_release() -> bool {
    crate::platform::current()
        .hotkey_capabilities()
        .reports_release
}

/// Moves every choice to an option this environment can actually use.
///
/// A setting that cannot work here must not stay selected: the user would see a
/// disabled option he cannot change away from, and the daemon would try to honour
/// something impossible.
pub fn normalise(form: &mut Value) -> bool {
    let mut changed = false;

    for field in Schema::load().fields() {
        if field.control != Control::Choice || field.options.is_empty() {
            continue;
        }

        let current = get_text(form, &field.key);
        let index = field.options.iter().position(|option| *option == current);

        let usable =
            index.is_some_and(|index| field.requirement(index).map(capability).unwrap_or(true));

        if usable {
            continue;
        }

        if let Some(index) = (0..field.options.len())
            .find(|index| field.requirement(*index).map(capability).unwrap_or(true))
        {
            set_text(form, &field.key, &field.options[index]);
            changed = true;
        }
    }

    changed
}

/// Writes a string.
pub fn set_text(form: &mut Value, key: &str, value: &str) {
    if let Some(slot) = slot_mut(form, key) {
        *slot = Value::String(value.to_string());
    }
}

/// Writes a boolean.
pub fn set_bool(form: &mut Value, key: &str, value: bool) {
    if let Some(slot) = slot_mut(form, key) {
        *slot = Value::Bool(value);
    }
}

/// Writes a number, keeping integers as integers so serde accepts them.
pub fn set_number(form: &mut Value, key: &str, value: f64, def: &FieldDef) {
    let value = def.max.map_or(value, |max| value.min(max));
    let value = def.min.map_or(value, |min| value.max(min));

    let encoded = if value.fract() == 0.0 {
        json!(value as i64)
    } else {
        json!(value)
    };

    if let Some(slot) = slot_mut(form, key) {
        *slot = encoded;
    }
}

/// Mutable slot at a dotted path.
fn slot_mut<'a>(form: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    let mut current = form;
    for segment in key.split('.') {
        current = current.get_mut(segment)?;
    }
    Some(current)
}

/// Evaluates `path == value`.
fn condition_holds(condition: &str, form: &Value) -> bool {
    let Some((path, expected)) = condition.split_once("==") else {
        return true;
    };
    get_text(form, path.trim()) == expected.trim().trim_matches('"')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn defaults() -> Value {
        to_form(&Settings::default())
    }

    #[test]
    fn the_descriptor_parses() {
        let schema = Schema::load();
        assert!(!schema.section.is_empty());
        assert!(schema.fields().count() > 15);
    }

    #[test]
    fn every_key_exists_in_the_settings() {
        let form = defaults();
        for field in Schema::load().fields() {
            assert!(
                get(&form, &field.key).is_some(),
                "{} does not exist in the settings",
                field.key
            );
        }
    }

    #[test]
    fn every_control_matches_the_field_type() {
        let form = defaults();
        for field in Schema::load().fields() {
            let value = get(&form, &field.key).expect("checked above");
            match field.control {
                Control::Text => assert!(
                    value.is_string(),
                    "{} is not text but the descriptor says so",
                    field.key
                ),
                Control::Number | Control::Slider => assert!(
                    value.is_number(),
                    "{} is not a number but the descriptor says so",
                    field.key
                ),
                Control::Switch => assert!(
                    value.is_boolean(),
                    "{} is not a boolean but the descriptor says so",
                    field.key
                ),
                Control::Keys | Control::Language | Control::Microphone | Control::Paragraph => {
                    assert!(
                        value.is_string(),
                        "{} is not a string but the descriptor says so",
                        field.key
                    );
                }
                Control::Choice => {
                    assert!(value.is_string(), "{} is not a string", field.key);
                    assert!(
                        !field.options.is_empty(),
                        "{} is a choice without options",
                        field.key
                    );
                    assert_eq!(
                        field.options.len(),
                        field.labels.len(),
                        "{} has mismatched options and labels",
                        field.key
                    );
                    assert!(
                        field.requires.is_empty() || field.requires.len() == field.options.len(),
                        "{} has mismatched options and requirements",
                        field.key
                    );
                }
            }
        }
    }

    #[test]
    fn an_option_that_cannot_work_here_is_never_selected() {
        let schema = Schema::load();
        let field = schema.field("hotkey.mode").expect("mode field");

        // The option that needs the extension, and the one that does not.
        let hold = field
            .options
            .iter()
            .position(|option| *option == "push-to-talk")
            .expect("push-to-talk");
        assert_eq!(field.requirement(hold), Some("hold-to-talk"));
        assert_eq!(field.requirement(0), None);

        let mut form = defaults();
        set_text(&mut form, "hotkey.mode", "push-to-talk");

        if !reports_release() {
            assert!(normalise(&mut form));
            assert_eq!(get_text(&form, "hotkey.mode"), "toggle");
        }
    }

    #[test]
    fn every_slider_has_bounds_and_a_step() {
        for field in Schema::load().fields() {
            if field.control != Control::Slider {
                continue;
            }
            let min = field.min.expect("slider without min");
            let max = field.max.expect("slider without max");
            let step = field.step.expect("slider without step");

            assert!(min < max, "{} has an empty range", field.key);
            assert!(step > 0.0, "{} has a non positive step", field.key);

            let value = get(&defaults(), &field.key)
                .and_then(Value::as_f64)
                .expect("slider over a number");
            assert!(
                (min..=max).contains(&value),
                "{} default {value} is outside {min}..={max}",
                field.key
            );
        }
    }

    #[test]
    fn every_choice_option_is_a_valid_value() {
        for field in Schema::load().fields() {
            if field.control != Control::Choice {
                continue;
            }
            for option in &field.options {
                let mut form = defaults();
                set_text(&mut form, &field.key, option);
                assert!(
                    from_form(&form).is_some(),
                    "{} does not accept the option {option}",
                    field.key
                );
            }
        }
    }

    #[test]
    fn no_setting_is_left_out_of_the_descriptor() {
        let mut leaves = Vec::new();
        collect_leaves(&defaults(), String::new(), &mut leaves);

        let schema = Schema::load();
        for leaf in leaves {
            assert!(
                schema.field(&leaf).is_some(),
                "{leaf} exists in the settings but not in settings.toml"
            );
        }
    }

    #[test]
    fn visibility_conditions_point_at_real_fields() {
        let form = defaults();
        for field in Schema::load().fields() {
            let Some(condition) = &field.visible_when else {
                continue;
            };
            let path = condition.split("==").next().unwrap_or_default().trim();
            assert!(
                get(&form, path).is_some(),
                "{} depends on {path}, which does not exist",
                field.key
            );
        }
    }

    #[test]
    fn the_endpoint_is_hidden_until_remote_mode_is_selected() {
        let schema = Schema::load();
        let endpoint = schema.field("asr.endpoint").expect("endpoint");

        let mut local = defaults();
        set_text(&mut local, "asr.mode", "sidecar");
        assert!(!endpoint.visible(&local));

        set_text(&mut local, "asr.mode", "remote");
        assert!(endpoint.visible(&local));
    }

    fn collect_leaves(value: &Value, prefix: String, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    collect_leaves(child, path, out);
                }
            }
            _ => out.push(prefix),
        }
    }
}
