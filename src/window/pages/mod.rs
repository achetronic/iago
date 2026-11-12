// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! One module per page. Each renders itself and mutates state through
//! [`gpui_kit::Context`] listeners.

pub mod about;
pub mod dependencies;
pub mod launch;
pub mod settings;
pub mod status;
