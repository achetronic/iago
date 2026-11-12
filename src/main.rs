// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Binary entry point. Every subcommand is a thin shell over the library.

use std::process::ExitCode;

use anyhow::Context;
use clap::Parser;
use iago::cli::{Cli, Command, CtlAction, DaemonAction};
use iago::config::ConfigStore;
use iago::ipc::{self, Action, Request, Response};
use iago::{daemon, logging, paths::Paths};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let log_file = Paths::resolve().ok().map(|paths| paths.log_file("iago"));
    logging::init_with_file(cli.verbose, log_file.as_deref());

    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("iago: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let paths = Paths::resolve()?;
    paths.ensure()?;
    let settings = ConfigStore::load(paths.config_file())?;

    match cli.command {
        // No subcommand: the window, which is what a person wants.
        None => open_control_center(),
        Some(Command::Ui) => run_window(),
        Some(Command::Daemon { action }) => match action {
            DaemonAction::Run => run_daemon(paths, settings),
            DaemonAction::Ambassador => ambassador(),
        },
        Some(Command::Ctl { action }) => control(&paths, action, &settings),
    }
}

/// Opens the control center in its own process.
///
/// Same binary, separate process: the window is a client of the daemon and a
/// crash in it must not take the daemon down.
fn open_control_center() -> anyhow::Result<ExitCode> {
    let executable = std::env::current_exe().context("cannot find my own binary")?;
    let mut child = std::process::Command::new(&executable)
        .arg("ui")
        .spawn()
        .with_context(|| format!("cannot start {}; run `make install`", executable.display()))?;

    child.wait()?;
    Ok(ExitCode::SUCCESS)
}

/// Runs the window in this process.
fn run_window() -> anyhow::Result<ExitCode> {
    iago::window::run();
    Ok(ExitCode::SUCCESS)
}

fn run_daemon(paths: Paths, settings: std::sync::Arc<ConfigStore>) -> anyhow::Result<ExitCode> {
    iago::start_async_io_reactor();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    let daemon = daemon::Daemon::new(settings, std::sync::Arc::new(paths));
    runtime.block_on(daemon.run())?;
    Ok(ExitCode::SUCCESS)
}

fn control(paths: &Paths, action: CtlAction, settings: &ConfigStore) -> anyhow::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    let response = runtime.block_on(ipc::call(
        &paths.socket(),
        &Request::new(Action::from(action)),
    ))?;
    print_response(&response, settings);

    Ok(if response.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Runs the ambassador: the work that cannot live inside the daemon.
///
/// The daemon starts it as its own process and replaces it when something has to
/// be released: the desktop ties a grabbed accelerator to the client that asked
/// for it, so replacing the process is what lets the combination change without
/// restarting Iago.
fn ambassador() -> anyhow::Result<ExitCode> {
    iago::start_async_io_reactor();
    iago::ambassador::run()?;
    Ok(ExitCode::SUCCESS)
}

fn print_response(response: &Response, settings: &ConfigStore) {
    let Some(state) = response.state.as_ref() else {
        // The smallest answer has nothing to paint: say how it went and stop.
        match &response.message {
            Some(message) => println!("{message}"),
            None if response.is_ok() => println!("ok"),
            None => println!("failed"),
        }
        return;
    };

    println!(
        "{}  {}",
        state.phase.label(),
        state
            .elapsed_secs
            .map(|secs| format!("{secs}s"))
            .unwrap_or_default()
    );

    if let Some(detail) = &state.detail {
        println!("  {detail}");
    }
    println!("  engine: {}", state.engine.label());
    // The mechanism is state; the combination and the mode are settings, so the
    // line is put together here instead of travelling glued in the answer.
    let hotkey = settings.snapshot().hotkey;
    match state.shortcut.as_ref() {
        Some(shortcut) => println!(
            "  shortcut: {} · {} · {}",
            shortcut.mechanism,
            hotkey.binding,
            hotkey.mode.label()
        ),
        None => println!("  shortcut: not bound"),
    }
    if let Some(text) = &state.last_text {
        println!("  last: {text}");
    }
    if let Some(message) = &response.message {
        println!("  {message}");
    }
    if !response.is_ok() {
        eprintln!("  request failed");
    }
}
