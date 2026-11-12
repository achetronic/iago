// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Iago control center.
//!
//! A window that is a pure client of the daemon: it renders a snapshot and posts
//! requests over the control socket. It links none of the platform machinery
//! (no portal, no clipboard, no simulated input), which is why it can be its own
//! process and its own package.

mod app;
mod bridge;
mod components;
mod mic;
mod pages;
mod prelude;
mod schema;
mod theme;

use std::sync::Arc;

use crate::config::ConfigStore;
use crate::paths::Paths;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::window::app::ControlCenter;

/// The window's assets: what gpui-kit ships, plus the mark.
///
/// `svg()` paints a single colour, which turns an illustration into a silhouette;
/// the mark is a PNG with transparency so the parrot keeps its own colours.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../../docs/img/logo/iago-mark.png"
            ))));
        }

        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// Name the mark is loaded by, and the file it comes from.
pub const MARK: &str = "iago-mark.png";

/// Whether the window should show the "Linux only for now" screen.
///
/// True on every platform without a port, and forced by `IAGO_SPLASH=1` so the
/// screen can be looked at while developing on Linux.
pub fn splash() -> bool {
    !crate::platform::supported() || std::env::var_os("IAGO_SPLASH").is_some()
}

/// Opens the control center. Blocking: runs the window until it is closed.
pub fn run() {
    let paths = Paths::resolve().expect("cannot resolve the data directories");
    paths.ensure().expect("cannot create the data directories");
    let settings = ConfigStore::load(paths.config_file()).expect("cannot load the settings");

    let paths = Arc::new(paths);

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);

        let settings = Arc::clone(&settings);
        let paths = Arc::clone(&paths);

        gpui_kit::open_window(window_options(cx), cx, move |window, cx| {
            cx.new(|cx| ControlCenter::new(settings, paths, window, cx))
        })
        .expect("cannot open the control center");
    });
}

fn window_options(cx: &mut App) -> WindowOptions {
    let splash = splash();
    let mut options = TitleBar::window_options();
    let bounds = if splash {
        size(px(620.0), px(430.0))
    } else {
        size(px(1020.0), px(700.0))
    };
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(None, bounds, cx)));
    options
}
