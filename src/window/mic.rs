// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Microphone dropdown.
//!
//! The settings page shows the capture sources of the running session as a
//! single select, which is what a user expects from a microphone picker.

use crate::audio::sources::Source;
use gpui_kit::SharedString;
use gpui_kit::component::{
    IndexPath,
    searchable_list::{SearchableListDelegate, SearchableListItem},
};

/// One row of the dropdown.
#[derive(Debug, Clone)]
pub struct SourceItem {
    /// PipeWire node name, empty for the system default.
    pub value: String,
    /// What the user reads.
    pub label: String,
}

impl SearchableListItem for SourceItem {
    type Value = String;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

/// The select's data source.
#[derive(Debug, Clone, Default)]
pub struct SourceDelegate {
    items: Vec<SourceItem>,
}

impl SourceDelegate {
    /// Builds the list with the system default first.
    pub fn new(sources: &[Source]) -> Self {
        let mut items = vec![SourceItem {
            value: String::new(),
            label: "System default".to_string(),
        }];

        items.extend(sources.iter().map(|source| SourceItem {
            value: source.name.clone(),
            label: source.description.clone(),
        }));

        Self { items }
    }

    /// Index of the item whose value matches, if any.
    pub fn position_of(&self, value: &str) -> Option<IndexPath> {
        self.items
            .iter()
            .position(|item| item.value == value)
            .map(IndexPath::new)
    }
}

impl SearchableListDelegate for SourceDelegate {
    type Item = SourceItem;

    fn items_count(&self, _section: usize) -> usize {
        self.items.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.items.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.items
            .iter()
            .position(|item| <Self::Item as SearchableListItem>::value(item) == value)
            .map(IndexPath::new)
    }
}
