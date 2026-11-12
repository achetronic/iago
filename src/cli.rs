// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Command line surface.
//!
//! The control center is the default: running `iago` with no arguments opens
//! the window. Everything else is a subcommand, mostly thin wrappers over
//! [`crate::ipc`].

use clap::{ArgAction, Parser, Subcommand};

/// Private, local dictation powered by Parakeet.
#[derive(Debug, Parser)]
#[command(
    name = "iago",
    version,
    about = "Private, local dictation for modern desktops",
    long_about = None
)]
pub struct Cli {
    /// Increase log verbosity. Repeat for trace level.
    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands.
///
/// Two nouns and nothing else: the daemon, and one way to talk to it. Running
/// `iago` with no command opens the window; everything the daemon can be asked to
/// do is a value of `ctl`, not a command of its own.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// The window itself. Running `iago` with no command opens it too.
    Ui,
    /// The process that owns dictation, the model server and the shortcut.
    Daemon {
        /// What to do: run it, or the helper it starts for itself.
        #[arg(value_enum, default_value = "run")]
        action: DaemonAction,
    },
    /// Send one command to the daemon over its control socket.
    Ctl {
        /// What to ask for.
        #[arg(value_enum, default_value = "recording-toggle")]
        action: CtlAction,
    },
}

/// What to do with the daemon process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DaemonAction {
    /// Run it in the foreground.
    Run,
    /// The helper the daemon starts when something has to live outside it.
    #[value(hide = true)]
    Ambassador,
}

/// Everything that is a command to the daemon.
///
/// The names are the wire names: `recording-toggle` here is
/// `{"action":"recording-toggle"}` there. One less translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CtlAction {
    /// Start or stop, depending on what the daemon is doing.
    RecordingToggle,
    /// Start recording.
    RecordingStart,
    /// Stop recording, transcribe and deliver.
    RecordingStop,
    /// Whether the daemon answers at all.
    DaemonAlive,
    /// Phase, engine, shortcut and last transcript.
    DaemonStatus,
    /// Re-read the settings file.
    SettingsReload,
    /// The key of the shortcut went down.
    ShortcutPress,
    /// The key of the shortcut came up.
    ShortcutRelease,
}

impl From<CtlAction> for crate::ipc::Action {
    fn from(action: CtlAction) -> Self {
        match action {
            CtlAction::RecordingToggle => Self::RecordingToggle,
            CtlAction::RecordingStart => Self::RecordingStart,
            CtlAction::RecordingStop => Self::RecordingStop,
            CtlAction::DaemonAlive => Self::DaemonAlive,
            CtlAction::DaemonStatus => Self::DaemonStatus,
            CtlAction::SettingsReload => Self::SettingsReload,
            CtlAction::ShortcutPress => Self::ShortcutPress,
            CtlAction::ShortcutRelease => Self::ShortcutRelease,
        }
    }
}
