// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Microphone discovery.
//!
//! The settings page cannot ask the user for a PipeWire node name: they do not
//! know one. `pw-dump` prints the graph as JSON, which is the least intrusive
//! way to list real capture sources without linking libpipewire.

use std::process::Command;

use serde::Deserialize;

/// One capture source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// PipeWire node name, what the capture stream targets.
    pub name: String,
    /// Human description, what the user reads.
    pub description: String,
}

/// Lists the audio sources of the running session, default first.
///
/// Returns an empty list when `pw-dump` is missing or the graph cannot be read;
/// the settings page then shows only the default option.
pub fn sources() -> Vec<Source> {
    let Ok(output) = Command::new("pw-dump").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    parse(&String::from_utf8_lossy(&output.stdout))
}

/// Extracts the sources from a `pw-dump` payload.
fn parse(raw: &str) -> Vec<Source> {
    #[derive(Deserialize)]
    struct Object {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        info: Option<Info>,
    }

    #[derive(Deserialize)]
    struct Info {
        #[serde(default)]
        props: Option<Props>,
    }

    #[derive(Deserialize)]
    struct Props {
        #[serde(rename = "media.class", default)]
        media_class: Option<String>,
        #[serde(rename = "node.name", default)]
        node_name: Option<String>,
        #[serde(rename = "node.description", default)]
        node_description: Option<String>,
    }

    let Ok(objects) = serde_json::from_str::<Vec<Object>>(raw) else {
        return Vec::new();
    };

    let mut sources: Vec<Source> = objects
        .into_iter()
        .filter(|object| object.kind == "PipeWire:Interface:Node")
        .filter_map(|object| object.info?.props)
        .filter(|props| props.media_class.as_deref() == Some("Audio/Source"))
        .filter_map(|props| {
            let name = props.node_name?;
            let description = props.node_description.unwrap_or_else(|| name.clone());
            Some(Source { name, description })
        })
        .collect();

    sources.sort_by(|a, b| {
        a.description
            .to_lowercase()
            .cmp(&b.description.to_lowercase())
    });
    sources.dedup_by(|a, b| a.name == b.name);
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_capture_sources_are_listed() {
        let payload = r#"[
            {"type": "PipeWire:Interface:Node",
             "info": {"props": {"media.class": "Audio/Source", "node.name": "mic_b", "node.description": "Mic B"}}},
            {"type": "PipeWire:Interface:Node",
             "info": {"props": {"media.class": "Audio/Sink", "node.name": "speakers", "node.description": "Speakers"}}},
            {"type": "PipeWire:Interface:Node",
             "info": {"props": {"media.class": "Audio/Source", "node.name": "mic_a", "node.description": "Mic A"}}},
            {"type": "PipeWire:Interface:Client", "info": {"props": {"node.name": "client"}}}
        ]"#;

        let sources = parse(payload);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].description, "Mic A");
        assert_eq!(sources[1].name, "mic_b");
    }

    #[test]
    fn a_broken_payload_is_not_fatal() {
        assert!(parse("not json").is_empty());
        assert!(parse("[]").is_empty());
    }

    #[test]
    fn a_source_without_a_description_falls_back_to_its_name() {
        let payload = r#"[
            {"type": "PipeWire:Interface:Node",
             "info": {"props": {"media.class": "Audio/Source", "node.name": "mic"}}}
        ]"#;

        let sources = parse(payload);
        assert_eq!(sources[0].description, "mic");
    }
}
