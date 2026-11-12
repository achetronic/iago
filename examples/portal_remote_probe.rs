// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Asks the RemoteDesktop portal for a keyboard session and prints every answer.
//!
//! It exists to see the error the input library swallows: when `enigo` cannot
//! open a connection it reports "no protocol available to simulate input" and
//! nothing else, which says nothing about why the portal refused.

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::remote_desktop::{
    DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
};

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let proxy = match RemoteDesktop::new().await {
            Ok(proxy) => proxy,
            Err(err) => return println!("proxy: {err}"),
        };
        println!("proxy: ok");

        let session = match proxy.create_session(CreateSessionOptions::default()).await {
            Ok(session) => session,
            Err(err) => return println!("create_session: {err}"),
        };
        println!("create_session: ok");

        match proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default().set_devices(DeviceType::KEYBOARD),
            )
            .await
        {
            Ok(_) => println!("select_devices: ok"),
            Err(err) => return println!("select_devices: {err}"),
        }

        match proxy.start(&session, None, StartOptions::default()).await {
            Ok(_) => println!("start: ok"),
            Err(err) => println!("start: {err}"),
        }
    });

    Ok(())
}
