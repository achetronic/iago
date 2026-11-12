// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Clipboard access as a long-lived actor.
//!
//! On Wayland the process that owns the selection must stay alive to serve it,
//! so the clipboard handle lives in a dedicated thread for the whole daemon
//! lifetime. Creating it per delivery would lose the content as soon as the
//! call returns.

use std::sync::mpsc::{self, Sender};

use crate::error::{Error, Result};

use super::Clipboard;

/// Clipboard backed by `arboard`, owned by one thread.
pub struct ArboardClipboard {
    commands: Sender<Command>,
}

enum Command {
    Read(Sender<Result<Option<String>>>),
    Write(String, Sender<Result<()>>),
}

impl ArboardClipboard {
    /// Spawns the owner thread. It connects lazily, on first use.
    pub fn new() -> Result<Self> {
        let (commands, inbox) = mpsc::channel::<Command>();

        std::thread::Builder::new()
            .name("iago-clipboard".to_string())
            .spawn(move || {
                let mut clipboard: Option<arboard::Clipboard> = None;

                for command in inbox {
                    match command {
                        Command::Read(reply) => {
                            let outcome = with_clipboard(&mut clipboard, |cb| {
                                Ok(cb.get_text().ok().filter(|text| !text.is_empty()))
                            });
                            let _ = reply.send(outcome);
                        }
                        Command::Write(text, reply) => {
                            let outcome = with_clipboard(&mut clipboard, |cb| {
                                cb.set_text(text.clone()).map_err(|err| {
                                    Error::Delivery(format!("clipboard write failed: {err}"))
                                })
                            });
                            let _ = reply.send(outcome);
                        }
                    }
                }
            })
            .map_err(|err| Error::Delivery(format!("cannot start the clipboard thread: {err}")))?;

        Ok(Self { commands })
    }

    fn call<T>(&self, build: impl FnOnce(Sender<Result<T>>) -> Command) -> Result<T> {
        let (reply, inbox) = mpsc::channel();
        self.commands
            .send(build(reply))
            .map_err(|_| Error::Delivery("the clipboard thread is gone".to_string()))?;
        inbox
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| Error::Delivery("the clipboard thread did not answer".to_string()))?
    }
}

impl Clipboard for ArboardClipboard {
    fn read(&self) -> Result<Option<String>> {
        self.call(Command::Read)
    }

    fn write(&self, text: &str) -> Result<()> {
        self.call(|reply| Command::Write(text.to_string(), reply))
    }
}

/// Runs `action` against a connected clipboard, opening it on first use.
fn with_clipboard<T>(
    slot: &mut Option<arboard::Clipboard>,
    action: impl FnOnce(&mut arboard::Clipboard) -> Result<T>,
) -> Result<T> {
    if slot.is_none() {
        let clipboard = arboard::Clipboard::new()
            .map_err(|err| Error::Delivery(format!("cannot reach the clipboard: {err}")))?;
        *slot = Some(clipboard);
    }

    match slot.as_mut() {
        Some(clipboard) => action(clipboard),
        None => Err(Error::Delivery("the clipboard is unavailable".to_string())),
    }
}
