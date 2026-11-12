// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Sends one fixed string through the delivery path in the settings.
//!
//! The transcript normally comes from the model, which makes a delivery bug slow
//! to reproduce. This takes the text from the command line instead, through the
//! same `text_sink` the daemon builds, so what lands in the focused window is
//! only the delivery's doing.
//!
//! Run it with `cargo run --example paste_probe -- "texto con acentos: é ñ @"`.

use iago::config::ConfigStore;
use iago::paths::Paths;
use iago::platform;

fn main() -> anyhow::Result<()> {
    let text = std::env::args().nth(1).unwrap_or_else(|| {
        "Hay una frontera muy clara entre un rosquete de Navidad hecho con desgana y una auténtica obra maestra."
            .to_string()
    });

    iago::logging::init(2);

    let paths = Paths::resolve()?;
    let settings = ConfigStore::load(paths.config_file())?;
    let snapshot = settings.snapshot();
    println!("target: {:?}", snapshot.delivery.target);

    let sink = platform::current().text_sink(&snapshot.delivery)?;
    let delivered = sink.deliver(&text)?;
    println!(
        "delivered {} chars with {}",
        delivered.chars, delivered.method
    );
    Ok(())
}
