// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! One import for the whole UI.
//!
//! `gpui_kit::component` is a namespace of modules, not a flat re-export, so the
//! components every page needs live here once.

pub use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, StyledExt as _,
};
pub use gpui_kit::component::{
    ActiveTheme as _, IconName, TitleBar,
    button::*,
    form::{Field, Form},
    group_box::GroupBox,
    input::{Input, InputEvent, InputState, NumberInput, Textarea, TextareaState},
    select::{Select, SelectEvent, SelectState},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    switch::Switch,
};
pub use gpui_kit::{KeyDownEvent, Keystroke, *};
